# Encore for Android: native app plan

Status: proposal, 2026-10-13. Scope: Android phone + Android Auto only. Desktop (Tauri, `clients/app`) and iOS are untouched.

Sources this plan is built on: the existing Android code (`clients/app/plugins/media/android/.../{CarService,PlaybackService,MediaPlugin,EncoreApi}.kt`, `clients/app/src/*.rs`), the server router (`server/src/main.rs`, `server/src/routes/*.rs`, `server/src/auth.rs`, `server/src/schema.sql`), the web player (`web/js/player.js`, `web/js/playback.js`, `web/js/remote.js`, `web/js/app.js` routes) and the Cadenza engine (`/home/user/cadenza/crates/cadenza-dsp`).

---

## 1. Goals and non-goals

**Goals**
- A fully native Kotlin app: native UI (Compose), native player (Media3), with Cadenza running on the phone and playing straight to the DAC.
- Mobile listening first: fast cold start, gapless, lock screen and Android Auto controls, survives bad networks (adaptive quality ladder), plays offline.
- Feature parity with the web app for listeners: library, search, discover, for you, requests, playlists, likes, remote devices, full-screen player, signal path, playback preferences (quality, effects, crossfade), lyrics, share links, offline, Android Auto.
- Light on the server. Prefer original files (Range-cacheable, no ffmpeg). Transcode only when the codec or the network needs it. Cache metadata locally. No polling loops while idle. Server-side Cadenza stays optional and admin-gated; the phone runs its own.
- Upgrade in place over the current Tauri APK, with the same package name and signing key.

**Non-goals (for now)**
- Admin screens: users, invites, settings, tagging, autodownload, diagnostics, storage, soulseek. The app shows "Open in browser" for these.
- A Play Store listing (see decision D3), tablets/foldables beyond basic adaptive layout, Wear OS, Cast.
- An exclusive libusb USB driver. Bit-perfect uses the platform `AudioMixerAttributes` API only.
- Rewriting the desktop Tauri app.

---

## 2. Architecture

### 2.1 Identity, location and upgrade path
- **New Gradle project at `clients/android/`.** It is a separate build from `clients/app` (Tauri), so the desktop build is unaffected.
- **applicationId stays `app.encore.client`.** Decision: keep it. Reasons:
  - Existing users get the native app as a normal update.
  - Android Auto and notification permissions carry over.
  - The `EncoreAndroid` User-Agent keeps working for the Sessions page.
  - The Kotlin **namespace** (code package) is new, `app.encore.android`, because namespace and applicationId are independent in AGP. The old `app.encore.client.media` code is ported, not linked.
- **versionCode** keeps the YYMMDDNN scheme from `apps.yml` and `scripts/release-apps.sh`, so the native build is always higher than the last Tauri build.
- **Signing continuity:**
  - An in-place upgrade only works if the native APK is signed with the **same certificate** as the APK on the user's phone.
  - If the `ANDROID_KEYSTORE_*` secrets were set for past releases, reuse that keystore. If past APKs were signed with a CI runner's throw-away debug key (the fallback in `apps.yml`), no key can continue them. Users then uninstall and reinstall once, and from then on the new release key is permanent (decision D1).
  - From M1 on, a release build **fails** without the keystore secrets. There is no silent debug-key fallback.
  - The keystore is stored as `ANDROID_KEYSTORE_BASE64` and decoded to a file at build time. Today `storeFile = file(ks)` treats the secret value as a path, which is a bug.
- **Data migration from the Tauri app:** none is needed. Tauri `config.json` servers and the WebView cookie are not readable in a useful way, so the native app opens on its own server picker. The URL saved in SharedPreferences `"encore"`/`"server"` (written by `MediaPlugin.setServer`) is pre-filled as a suggestion, and the user signs in once. Offline files in `$APPDATA/offline/` are deleted on first run (one notice: "offline songs need to be downloaded again").

### 2.2 Modules (`clients/android/`)
```
settings.gradle.kts, gradle/libs.versions.toml, build-logic/ (convention plugins)
app/                      MainActivity, EncoreApp (Hilt), NavHost, deep links, share target
core/model/               @Serializable DTOs mirroring server JSON (Track, Album, Artist, Playlist, Request, User, Prefs, RemoteState …)
core/network/             OkHttp client + AuthInterceptor + Retrofit services (EncoreApi*), SSE client, ServerProbe
core/data/                Room 3 DB, repositories, PrefsRepository (server /api/auth/prefs + local), SessionStore
core/designsystem/        EncoreTheme (colors/typography/shapes), shared composables (Cover, TrackRow, AlbumCard, Sheet)
playback/                 EncorePlaybackService (MediaLibraryService), EncorePlayer, BrowseTree, QualityPolicy,
                          StreamResolver, Scrobbler, ResumeStore, CrossfadePlayer, UsbOutputManager, SignalPath
cadenza/                  Kotlin facade (CadenzaEngine, CadenzaAudioProcessor) + JNI .so built by cargo-ndk
cadenza/rust/             crate `encore-cadenza-jni` (cdylib) depending on cadenza-dsp
remote/                   RemoteDevice (acts as a device), RemoteController (controls others)
offline/                  EncoreDownloadService, DownloadRepository, OfflinePlayLog
feature/auth|home|library|search|discover|foryou|playlists|requests|player|lyrics|devices|settings|account/
```
- Dependency direction: `feature/*` → `core/*`, `playback` (controller side only). `playback` → `core/*`, `cadenza`, `remote`, `offline`.
- **One playback service** serves the phone UI and Android Auto. The UI connects with a Media3 `MediaController` (via `SessionToken`). There is no second player and no WebView. Media3 provides the notification and lock-screen controls, so `PlaybackService.kt` and the "pause the other player" dance (`MediaPlugin.control`/`pauseFromWebView`) disappear.

---

## 3. Tech stack (exact versions)

| Area | Choice | Version |
|---|---|---|
| Language | Kotlin (AGP 9 built-in Kotlin; do **not** apply `org.jetbrains.kotlin.android`) | 2.4.21 |
| Build | AGP / Gradle wrapper | 9.4.0 / 9.6.1 (the repo already uses 9.6.1; AGP 9.4 needs ≥ 9.6.0) |
| Annotation processing | KSP | 2.3.12 |
| JDK | Temurin | 21 |
| UI | Compose BOM | 2026.09.00 (Compose 1.12.1, material3 1.4.0) |
| Material 3 Expressive | material3 | stay on 1.4.0 from the BOM; move to 1.5.0 stable when it ships (1.5.0-beta01 is out) |
| Navigation | navigation-compose, type-safe routes | from the BOM's compatible line (pin at M1) |
| Images | Coil 3 (`io.coil-kt.coil3:coil-compose`, `coil-network-okhttp`) | 3.6.3 |
| Media | Media3 `exoplayer`, `session`, `datasource-okhttp`, `ui-compose`, `ui-compose-material3`, `exoplayer-dash` not needed | 1.11.1 |
| HTTP | OkHttp (+ `okhttp-sse`) | 5.5.0 |
| REST | Retrofit + kotlinx-serialization converter | 3.0.0 |
| JSON | kotlinx.serialization | 1.11.0 |
| Async | kotlinx.coroutines | 1.11.0 |
| DB | Room 3 (`androidx.room3`, bundled SQLite driver) | 3.0.3 |
| Background | WorkManager | 2.12.0 |
| DI | Hilt (KSP) | 2.60.1 |
| Prefs / secrets | DataStore + Tink AEAD (Android Keystore master key) | pin latest stable at M1 (not verified here) |
| Rust → Android | cargo-ndk / jni crate / NDK | 4.1.2 / 0.22.4 / 29.0.13846066 |
| SDK | minSdk / target / compile | **26** / 36 / 36 (decision D4) |
| ABIs | arm64-v8a, armeabi-v7a (+ x86_64 for debug/emulator only) | |

Why OkHttp + Retrofit over Ktor: one `OkHttpClient` (with the auth interceptor) is shared by Retrofit, Coil, the SSE client and Media3's `OkHttpDataSource.Factory`. Auth, User-Agent, the cache and connection pooling are then the same for API calls, covers and audio streams.

Every request sends `User-Agent: EncoreAndroid/<versionName> (Android <sdk>; <model>)`. The server's `device_name()` already maps `EncoreAndroid` to "Encore app on Android".

---

## 4. Auth design

### 4.1 Today (works with no server change)
- POST `/api/auth/login {username, password, code?}`, then read `Set-Cookie: encore_session=…` from the response and store the token.
- Send it as `Cookie: encore_session=<t>`, plus `X-Requested-With: encore` on every non-GET (CSRF).
- 2FA: a `401 {two_factor:true}` reply means show the code field and resubmit with `code`.
- Downside: the session expires on a fixed date after 30 days, there is no refresh, and the token is indistinguishable from a browser session.

The app uses this path automatically when the server lacks the `app_tokens` feature, so M1 works against any current server.

### 4.2 Server change S1: app tokens (bearer)
**DB** (`server/src/db.rs::migrate`, additive `ALTER TABLE` like the existing ones; documented in `schema.sql`):
```sql
ALTER TABLE sessions ADD COLUMN kind TEXT NOT NULL DEFAULT 'web';   -- 'web' | 'app'
ALTER TABLE sessions ADD COLUMN device_id TEXT;                      -- stable per install (UUID the app generates)
ALTER TABLE sessions ADD COLUMN device_name TEXT;                    -- "Pixel 9 Pro"
ALTER TABLE sessions ADD COLUMN last_seen_at REAL;
CREATE INDEX IF NOT EXISTS sessions_user_device ON sessions(user_id, device_id);
```
- For `kind='app'`, the `token` column stores `"sha256:" + hex(sha256(token))`. The raw token is shown to the client once and never stored. Web sessions are unchanged.
- **Lifetime:** `app_session_days` setting, default 365, **sliding**. When `last_seen_at` is more than 24 h old, a request updates `last_seen_at` and pushes `expires_at` forward. This is at most one write per device per day, so it never hammers the DB.

**Endpoints:**
- `POST /api/auth/login`, `/api/auth/signup`, `/api/auth/magic` accept an optional `app: {device_id, device_name}`.
  - When present: no cookie is set, and the body is `{user, token}`.
  - The same `device_id` replaces that device's previous app session, so re-login doesn't pile up rows.
- `server/src/auth.rs::user_from_parts` accepts `Authorization: Bearer <token>` (hash, look up `kind='app'`).
  - It is **CSRF-exempt**, like `X-API-Key`, because bearer headers are never sent by a browser on its own.
  - Expired, disabled-user or missing tokens return 401 → the app goes to the sign-in screen.
- `GET /api/auth/sessions` adds `kind`, `device_name`, `last_seen_at` to each row.
  - The Sessions page (web `account.js` and the native Account screen) shows "Encore app · Pixel 9 Pro · last used 2 h ago".
  - `DELETE /api/auth/sessions/{sid}` revokes it (sid = first 10 chars of the stored value, as today). `logout-all`, a password change and a password reset already delete all sessions, app tokens included.
- `POST /api/auth/logout` with a bearer token deletes that row.
- `GET /api/auth/info` adds `"app_tokens"` to `features[]`, plus `api: 2` (a new integer API level the app gates features on; server change S12).

**Client:**
- `SessionStore` keeps `{serverUrl, token, kind: bearer|cookie, userId}` in DataStore, encrypted with a Tink AEAD key held in the Android Keystore. (`EncryptedSharedPreferences` is deprecated.)
- `AuthInterceptor` adds the header.
- A 401 on any call clears the session and emits `AuthState.SignedOut`.
- The playback service returns `RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED` to Auto, as `CarService` does today.

**Email links (S2, M3):** the magic-link and reset emails point to web hash routes (`/#/magic?token=`), which App Links cannot match. Add `GET /app/magic?token=` and `/app/reset?token=` paths that redirect to the hash route in a browser, and declare them as an `autoVerify` intent filter for the server host. Self-hosted servers can't publish `assetlinks.json` for our app reliably, so also accept a **6-digit code** in the same email: `POST /api/auth/magic {code, login}`. The app uses the code entry; the link stays for browsers.

---

## 5. Data layer, caching, offline

- **Room 3 DB `encore.db`** (one per server+user, `encore-<hash(server,user)>.db`). Tables:
  - `albums`, `tracks`, `artists`, `playlists`, `playlist_items`, `likes(track_id)`
  - `recent_plays`, `requests`
  - `offline_tracks(track_id, quality, bytes, state, added_at)`
  - `pending_plays(track_id, seconds, played_at)` and `pending_mutations(id, kind, payload_json, created_at)`
  - `kv(key, json, fetched_at)` for one-off screens (discover, for-you, stats).
- **Pattern:** the UI reads `Flow`s from Room only, and repositories refresh from the network (stale-while-revalidate).
- **Freshness budgets (server load):**
  - albums/artists list: 6 h, or pull-to-refresh
  - album detail: 1 h
  - playlists: on open (compare `updated_at`)
  - likes: on app foreground, at most 1/min
  - discover/for-you: 12 h
  - requests: 15 s **only while the Requests screen is visible and any request is active**, otherwise never.
- **Paging:**
  - albums (`limit/offset`, `total`) via Paging 3 `RemoteMediator` into Room.
  - tracks (no `total`; stop when a short page comes back).
  - artists (single call, capped at 2000 on the server; server change S7 adds paging).
- **Writes** are applied optimistically to Room and queued in `pending_mutations`. A `SyncWorker` (WorkManager, `CONNECTED`) flushes them: likes, playlist add/remove/move, requests.
- **HTTP cache:** OkHttp `Cache` 50 MB for JSON. Covers go through Coil's disk cache (250 MB) with fixed sizes **s=96, 300, 600, 1200** only, so the server's thumbnail cache (`data/covers/thumbs/{album}_{N}.jpg`) stays small.
- **Offline (M5): Media3 downloads.**
  - `EncoreDownloadService : DownloadService` (foregroundServiceType `dataSync`) with a `WorkManagerScheduler`.
  - Downloads go into `SimpleCache(filesDir/offline, NoOpCacheEvictor)`, separate from the streaming cache `SimpleCache(cacheDir/stream, LeastRecentlyUsedCacheEvictor(512 MB))`.
  - Playback uses a `CacheDataSource` that checks the offline cache first, then the stream cache, then the network.
  - Offline quality: **Original** uses `/api/library/tracks/{id}/file`, which has Range so it is resumable and needs no ffmpeg on the server. **Compact** uses `/stream?q=aac&kbps=256`, which is a pipe, so it restarts from zero on failure.
  - Constraints: unmetered by default, `maxParallelDownloads = 2`.
  - Album/playlist "Download" adds all tracks. Covers are pinned in Coil's cache at s=600.
- **Offline play log:** plays are written to `pending_plays` with the real `played_at` and uploaded by server change S3 (batch with timestamps). Until S3 exists, they upload one by one with the current endpoint (the timestamp is then "now", and the app notes this as a limitation).

---

## 6. Playback engine

### 6.1 Service
- `EncorePlaybackService : MediaLibraryService` is a port of `CarService.kt`. It keeps:
  - the browse tree (`BrowseTree`, ported 1:1 from `CarService.Tree`)
  - voice search, `onPlaybackResumption`
  - the scrobble rules (`Scrobbler`: now-playing on change; play at min(duration/2, 240 s); <30 s tracks at the end)
  - the transcode set (`ape wv aiff aif wma alac dsf dff tta mpc`; `alac` only when MediaCodec lacks it)
  - the `track:{id}|{parent}` media IDs.
- Manifest:
  - `foregroundServiceType="mediaPlayback"`, exported, with both `androidx.media3.session.MediaLibraryService` and `android.media.browse.MediaBrowserService` actions.
  - `automotive_app_desc.xml` copied over.
  - `MediaSession.Callback.onConnectAsync` (Media3 1.11) waits for `SessionStore` to load.
  - Untrusted controllers get read-only commands. Auto, the system UI and our own app get full access.
- The player stack inside the service:
  ```
  MediaSession ── EncorePlayer (ForwardingSimpleBasePlayer)
                    └─ CrossfadePlayer (M4: two ExoPlayers A/B; M1–M3: one ExoPlayer)
                          └─ ExoPlayer ── DefaultRenderersFactory(float output on)
                                └─ DefaultAudioSink(audioProcessors = [CadenzaAudioProcessor])
                                └─ DefaultMediaSourceFactory(CacheDataSource → ResolvingDataSource(StreamResolver) → OkHttpDataSource)
  ```
  `EncorePlayer` is the only thing the session sees, so the internals can change between milestones without touching the UI or Auto.

### 6.2 Stream selection (`StreamResolver`, `QualityPolicy`)
- The URL is built when each item starts loading, so the preloaded next item gets the quality current at that moment.
  - Original: `/api/library/tracks/{id}/stream`
  - Forced FLAC: `?transcode=1` for formats in the transcode set
  - Lossy: `?q=aac&kbps=N`. AAC is used rather than Opus for parity with the web player and reliable MediaCodec decoding.
- **Ladder**, ported from `web/js/playback.js`: `RUNGS = [0(original), 256, 160, 96, 64, 32]`, `SAVER = 96`, EWMA α = 0.3, choose the highest rung needing ≤ 75 % of the estimate.
  - Samples come from a `TransferListener` on the OkHttp data source (bytes/elapsed per transfer).
  - The seed estimate comes from `ConnectivityManager.getNetworkCapabilities().linkDownstreamBandwidthKbps`.
  - Modes: `auto | original | saver`, matching `playback.quality`.
- **Mid-track step-down:** on 2 rebuffers within 30 s, or one longer than 3 s, replace the current item with the next rung down at `t=<position>` (a pipe has no Range). The toast matches the web wording. Step-up happens only at the next track boundary, so there is no churn.
- **Gapless:**
  - Originals (FLAC/MP3 with LAME/M4A with iTunSMPB/Opus) are gapless in ExoPlayer within a playlist.
  - Transcoded pipes are not gapless. To keep albums gapless in Auto mode, the policy does not switch rungs inside an album run: consecutive tracks with the same `album_id` use the rung chosen for the first track unless a stall forces a step-down.
- **Preload:** `playback.preload=true` maps to `DefaultLoadControl` with a 60 s forward buffer, letting ExoPlayer pre-open the next item. With `false` the buffer is 15 s and there is no prefetch.
- **Autoplay:** when the queue ends and `playback.autoplay` is set, POST `/api/library/shuffle {limit: 50, exclude:[recent ids]}`.

### 6.3 Effects and Cadenza on the phone (`cadenza/`)
- **One DSP implementation:** all effects run in Cadenza (`cadenza-dsp`), the same code as the server and desktop engine. Nothing is re-implemented in Kotlin, and there is no `android.media.audiofx`.
- **Rust crate** `clients/android/cadenza/rust` (`encore-cadenza-jni`, `crate-type=["cdylib"]`):
  - Depends on `cadenza-dsp` via a git dependency pinned by `rev` on `https://github.com/Krifmaz/cadenza` (decision D2).
  - **JNI surface (hand-written, `jni` 0.22.4), hot path only:**
    - `nativeCreate(rate, bits, channels, configJson): Long`
    - `nativeProcess(handle, inBuf: ByteBuffer /*direct, f32 interleaved*/, frames, outBuf: ByteBuffer): Int`
    - `nativeFlush(handle, outBuf): Int`
    - `nativeReset(handle)`, `nativeSetVolume(handle, linear)`
    - `nativeLatencySeconds(handle): Double`
    - `nativeSignalPath(handle): String /*json*/`
    - `nativeCatalog(): String /*filters+dithers json*/`
    - `nativeDestroy(handle)`
  - The control plane is a JSON string. That is small and infrequent, so UniFFI isn't needed.
  - `CoefficientCache::in_dir(context.cacheDir/cadenza-fir)`, so long FIR designs are computed once per filter/rate.
- **Needed `cadenza-dsp` changes** (made in the cadenza repo, then the pinned `rev` is bumped):
  1. `Pipeline::process_into(&mut self, pcm: &[f64], out: &mut Vec<i32>)` and an f32 in/out variant. This avoids the per-call `Vec` allocation in `process()` on the audio thread.
  2. `crossfeed` (BS2B-style; off/light/medium/strong matching the web presets), `balance` (−100..100) and `mono` stages in `PipelineConfig`, placed after EQ and before the resampler.
  3. `PipelineConfig.preamp_db`, unless `Volume` already covers it.
- **`CadenzaAudioProcessor : BaseAudioProcessor` (Kotlin):**
  - Input is float PCM (Media3 `ToFloatPcmAudioProcessor` runs first). Output:
    - `C.ENCODING_PCM_FLOAT` at the source rate when only effects are on;
    - `C.ENCODING_PCM_32BIT` at the target rate when upsampling ("Cadenza on phone") is on and API ≥ 31; float otherwise.
  - When effects are off and upsampling is off it reports `isActive=false`, so the chain is a true passthrough.
  - Pipelines are rebuilt on format change (`onConfigure`).
  - Latency (`latency_seconds`) is reported so the position stays correct. This uses Media3's processor duration/offset hooks; check the exact 1.11 API name at M2.
  - Config comes from `PrefsRepository.playback`: `fx_on`, `eq_preset`/`eq_bands`/`eq_preamp`, `crossfeed`, `balance`, `mono`, and on-device `dsp_filter`/`dsp_dither`/`dsp_rate`.
- **EQ presets:** port `web/js/playback.js` preset band tables and `clean()` validation into `core/data/PlaybackPrefs.kt`, with unit tests checking identical output against JSON fixtures exported from playback.js. The server stays schema-less here until S10.
- **Threading:** the processor runs on ExoPlayer's playback thread. There are no allocations in `queueInput`, and direct ByteBuffers are reused. If profiling shows 1M-tap filters missing deadlines on mid-range phones, offer only "Short/Medium" filters below a CPU tier (`Runtime.availableProcessors()` + `PerformanceClass`).

### 6.4 Server-side Cadenza (stays optional)
- Shown only when `GET /api/playback/dsp` reports `enabled && allowed` (admin switch + `server_dsp` perm).
- Off by default in the app, because phone-side Cadenza is the default "upsampling" path.
- When it is on: `&dsp=1&rate=&filter=&dither=`, plus `POST /api/playback/dsp/prepare` for the next track. On a `503 busy`, fall back to the original silently (web behaviour). `x-encore-dsp*` headers feed the signal path.
- Phone Cadenza and server Cadenza are mutually exclusive per track. The server wins if both are switched on, and the settings screen explains this.

### 6.5 Crossfade (M4)
- `CrossfadePlayer : SimpleBasePlayer` owns ExoPlayer A and B and presents one timeline and queue to the session (Media3 has no built-in crossfade; androidx/media#2 is still open).
- When `crossfade > 0`, at `duration − crossfade` it prepares the next item on the idle player and ramps the volumes (equal-power curve, 20 ms steps).
- Off when `crossfade_album_gapless` is set and both tracks share an `album_id`: normal gapless handoff on the same player.
- Disabled while bit-perfect USB output is active.
- Scrobbling and remote state read from `EncorePlayer`, never from A/B directly.

### 6.6 Bit-perfect USB (M6, API 34+)
- `UsbOutputManager` watches `AudioDeviceCallback` for `TYPE_USB_DEVICE`/`TYPE_USB_HEADSET`.
- If `AudioManager.getSupportedMixerAttributes(device)` contains `MIXER_BEHAVIOR_BIT_PERFECT`, Settings shows **"Send directly to USB DAC (bit-perfect)"**.
- **Per track** (before prepare, and again on a rate/format change):
  - The target format is the source rate/bit depth, or Cadenza's output rate with 32-bit when phone Cadenza is on.
  - Pick the matching supported `AudioMixerAttributes`, then `setPreferredMixerAttributes(USAGE_MEDIA attrs, device, attrs)` and `ExoPlayer.setPreferredAudioDevice(device)`.
  - The `AudioTrack` must match exactly. If the default sink doesn't, use `DefaultAudioSink.Builder.setAudioOutputProvider` with a provider that builds the track with the mixer's encoding/rate/mask (check the API at M6).
- **In this mode:**
  - Effects are allowed only when Cadenza produces the final integer stream (the system mixer is untouched, which is the point).
  - Crossfade is off.
  - Volume is the DAC's hardware volume. If the DAC has none, use Cadenza's digital volume at 32-bit, with a visible note.
  - Gapless across different sample rates needs a reopen, and a short gap is accepted.
- On unplug or when the device is lost: `clearPreferredMixerAttributes`, pause (becoming-noisy), and the normal mixer is used again.
- If BIT_PERFECT isn't offered (many OEM builds), the app plays at the device's native rate through the mixer and the signal path says so honestly.
- Bluetooth is labelled "high quality, not bit-perfect".

### 6.7 Signal path (`SignalPath`)
- **Built from:**
  - the source: `GET /api/library/tracks/{id}/info` (codec, rate, bits, lossless), cached in Room;
  - the stream: `x-encore-stream`, `x-transcoded`, `x-cadenza-path`, `x-encore-dsp` response headers, captured by a network interceptor keyed by track id;
  - the phone: `CadenzaEngine.signalPath()` JSON, the active `AudioDeviceInfo` (type/product name), mixer behaviour (bit-perfect or mixed), and `AudioTrack` rate/encoding.
- Shown as a stepped list in the player sheet: Source → Stream → Phone DSP → Output, with a quality badge (Bit-perfect / Lossless / High quality / Lossy).
- The same `{label, transport, output_rate}` triple goes into the remote `signal` field.

---

## 7. Remote control (M4)
- **As a device (`RemoteDevice`):**
  - id `phone-<ANDROID_ID>` and name `"Phone · <MODEL>"`, or the user's rename. The old car id `car-<ANDROID_ID>` is retired, because there is now one service.
  - Opens SSE `GET /api/remote/events?device=<id>` with OkHttp SSE.
  - Heartbeat POST `/api/remote/heartbeat {id, name, state}`. State is the shape from `player.js` snapshot: `{id,title,artist,album,album_id,format,duration,cover,playing,position,volume,index,signal,queue[≤300 slim]}`.
  - **Load rule:** heartbeat immediately on change, and every **10 s** while SSE is connected (still inside the 15 s online window), or every 2 s only if SSE is down. Stop both 5 minutes after playback pauses with the app in the background; the device then drops off the list, as a closed browser tab does.
  - Commands are de-duplicated by `seq` (ring of 200) and executed on `EncorePlayer`: toggle, play, pause, stop, next, prev, seek, volume, enqueue, play_next, play_queue, and `engine{set}` mapped to phone Cadenza settings.
- **As a remote (`RemoteController`):**
  - The Devices sheet lists `GET /api/remote/devices` (live from the same SSE `devices` events while the sheet is open).
  - Picking a device switches the player UI to a `RemotePlayer : SimpleBasePlayer` adapter. It sends `POST /api/remote/{device}/command` and renders that device's state, so the same full-screen player UI drives local or remote.
  - "Transfer here" sends `play_queue` to the target with the current queue/index/position, then pauses locally.
- **Roon** (only if the `roon` feature is present): zones appear in the same Devices sheet via `/api/roon/zones`, with commands through `/api/roon/zones/{id}/command`.
- Doze will cut SSE in the background. That is acceptable, because heartbeats keep running while the playback service is in the foreground (it is when playing). Waking a paused phone remotely needs push (S11, later).

---

## 8. Screens (mapped to `web/js/app.js` routes)

| Native screen | Web route / file | Shows |
|---|---|---|
| ServerPicker, SignIn, TwoFactor, SignUp, ForgotPassword | `ui/index.html`, `pages/auth.js` | Saved servers, `/api/health` probe (https first), `/api/auth/info` (app name, signup mode, email login), login + TOTP, invite check `/api/auth/invite/{code}` |
| Home | `#/` `pages/home.js` | Recently added (`/api/library/recent`), recently played (`/api/library/plays/recent`), Made for you row, continue-listening |
| Search | `#/search` `pages/discover.js renderSearch` | Library results (tracks/albums/artists via `q`, or S5 unified) plus "More on MusicBrainz" (`/api/discover/search`, natural-language "understood" chip), request buttons |
| For you, Tag | `#/for-you`, `#/tag/*` `pages/foryou.js` | `/api/discover/for-you`, `/made-for-you`, `/new-releases`, tags |
| Artist (catalog), Album (catalog) | `#/artist/*`, `#/artist-name/*`, `#/album/*` | MB artist, library albums/tracks, catalog, similar, extras (bio, picture), request album/track |
| Library | `#/library` `pages/library.js` | Tabs Albums / Artists / Songs / Downloaded; sort + filter (lossless, singles/albums); stats header |
| Library album | `#/library/album/*` | Cover, tracks, play/shuffle, like, add to playlist, download album (offline, or the ZIP via system DownloadManager), share track |
| Playlists, Playlist | `#/playlists`, `#/playlists/{id}` `pages/playlists.js` | Liked first; create, rename, reorder (drag), remove, add tracks; import from URL/text (preview → import); unmatched items with "request missing" |
| Requests | `#/requests` `pages/requests.js` | Own requests with status/log, retry/cancel; new request from link (`/api/requests/link`); picker (`/api/picker/*`) only with the `pick` perm |
| Download (activity) | `#/download` `pages/download.js` | Read-only view of the user's active downloads/requests (admin parts hidden) |
| Leaderboard | `#/leaderboard` | `/api/stats/me`, `/api/stats/leaderboard` |
| Now playing (mini + full) | `player.js` | Big art, scrubber, like, queue, lyrics tab, signal path, devices button, quality badge, sleep timer, share |
| Queue | `player.js` | Reorder/remove, play next, save as playlist |
| Lyrics | `pages/lyrics.js` | `/api/library/tracks/{id}/lyrics`, synced LRC auto-scroll, tap a line to seek |
| Devices sheet | `remote.js` | Section 7 |
| Settings → Playback | `pages/settings.js` + `playback.js` | Quality (Auto / Data saver 96 / Original), preload, autoplay, effects (EQ graph + presets + preamp, crossfeed, balance, mono), crossfade + keep albums gapless, Phone Cadenza (filter, dither, rate), Server Cadenza (if allowed), USB bit-perfect (API 34+), offline quality + storage use |
| Settings → Appearance | `appearance.js` | Theme (Encore / OLED / Light / System), accent |
| Account | `#/settings?s=account` `pages/account.js` | Profile, avatar, password, 2FA setup (QR + `otpauth://` open), sessions/devices with revoke, connections list (read-only + "manage in browser"), sign out / sign out everywhere |
| Admin items | `admin.js`, `storage`, `soulseek`, … | A single "Server admin" row that opens the server in Custom Tabs |

Navigation: bottom bar **Home · Search · Library · Playlists · Requests**, with For you reached from Home. The mini player sits above the bar and expands to the full-screen player with a shared-element transition.

---

## 9. Theming
- `core/designsystem/Color.kt` takes its values from `web/css/app.css` `:root[data-theme="encore"]`:
  - `background #161617`, `surface #1c1c1e`, `surfaceContainer #222224`, `surfaceContainerHigh #2a2a2d`
  - `outline #353538`, `onBackground/onSurface #f0ead6`, `onSurfaceVariant #aaa493`, `outlineVariant #77725f`
  - `primary #f0ead6` (cream) with `onPrimary #161617`.
- Variants: OLED (`#000` bg, from `data-theme="oled"`) and Light (from `data-theme="light"`).
- No dynamic colour by default. The logo colours are the brand. "Match wallpaper" is offered as an option.
- Optional now-playing tint from album art via `androidx.palette`, clamped to keep contrast ≥ 4.5:1 against cream text.
- Typography: system sans with Material 3 type scale. Edge-to-edge throughout (targetSdk 36 enforces it).
- Launcher icon and notification small icon from the existing `clients/app/icons` (monochrome layer for themed icons).

---

## 10. Android Auto
- **Reuse:** `BrowseTree`, search, resumption and scrobbling come straight from `CarService.kt`. Paging and content-style extras are kept.
- Changes:
  - Root tabs ≤ 4: **Home** (recent + played + made for you), **Playlists** (liked first), **Library** (albums, artists), **Downloaded** (shown when offline or `LibraryParams.isOffline`).
  - "Shuffle all" is a playable item in Home.
  - Artwork as `content://` URIs from a `CoverProvider` (ContentProvider) backed by Coil's disk cache, as the car guidance recommends. Today's direct authenticated HTTP fetch is replaced.
  - Per-item command buttons: Like and Play next (`setCommandButtonsForMediaItems`). Transport extras: Like and Shuffle (`setMediaButtonPreferences`).
  - Auth errors use `LibraryResult` error codes with a resolution `PendingIntent` that opens SignIn on the phone.
- Test with the DHU and the AAOS emulator against the car-app quality guidelines.
- **Sideloaded caveat:** Android Auto hides apps not installed from Play unless "Unknown sources" is enabled in Auto's developer settings. This is the same as today (decision D3).

---

## 11. CI and release
- `.github/workflows/apps.yml`:
  - Add job **`android-native`** (ubuntu-latest). It is dispatch-only, like the rest of the file: `workflow_dispatch` plus `workflow_call` from `release.yml` when the `apps` input is ticked. The `release.yml` trigger is not changed.
  - The old `android` (Tauri) job keeps running until M1 is released, then it is deleted in the same PR that removes the Android target from `clients/app` (M7).
- Steps:
  1. `actions/checkout@v4`
  2. `actions/setup-java@v4` temurin 21, with gradle cache via `gradle/actions/setup-gradle@v4`
  3. `android-actions/setup-android@v3` with `platform-tools platforms;android-36 build-tools;36.0.0 ndk;29.0.13846066`
  4. `dtolnay/rust-toolchain@stable` targets `aarch64-linux-android,armv7-linux-androideabi,x86_64-linux-android`, plus `Swatinem/rust-cache@v2` (workspace `clients/android/cadenza/rust`)
  5. `cargo install cargo-ndk --version 4.1.2 --locked`
  6. If the cadenza repo is private: `webfactory/ssh-agent` with a read-only deploy key secret `CADENZA_DEPLOY_KEY` (decision D2)
  7. Decode `ANDROID_KEYSTORE_BASE64` to `$RUNNER_TEMP/release.jks`, and **fail** if any of the 4 signing secrets is missing
  8. `./gradlew -p clients/android :app:assembleRelease :app:bundleRelease -PencoreVersionName=${{inputs.semver}} -PencoreVersionCode=${{inputs.code}}`
  9. Upload `Encore-android-<version>.apk` (same artifact name as today, so `release.yml` attaches it unchanged)
- Gradle wiring: `cadenza/build.gradle.kts` registers `cargoNdkBuild` (Exec: `cargo ndk -t arm64-v8a -t armeabi-v7a [-t x86_64 for debug] -o build/jniLibs build --release`), adds `build/jniLibs` as a jniLibs source dir, and makes `preBuild` depend on it.
- R8 is on, with keep rules for JNI (`-keepclasseswithmembernames class * { native <methods>; }`) and kotlinx.serialization.
- Per-PR checks: a new `android-check` job on `pull_request` paths `clients/android/**` running `./gradlew lint testDebugUnitTest` plus `cargo test` for the JNI crate. It runs on push to PRs only, and is cheap on Linux.
- `scripts/release-apps.sh --android` switches to the Gradle build (same versioning, same `edge` upload).

---

## 12. Server-side changes

| # | Change | Files | Needed by |
|---|---|---|---|
| S1 | App tokens: `sessions.kind/device_id/device_name/last_seen_at`, hashed bearer, sliding expiry, `app` field on login/signup/magic, `Authorization: Bearer` in `user_from_parts` (CSRF-exempt), sessions list fields, `app_tokens` feature | `db.rs`, `schema.sql`, `auth.rs`, `routes/auth.rs`, `web/js/pages/account.js` | M1 (optional; cookie fallback) |
| S2 | `/app/magic`, `/app/reset` redirects + 6-digit emailed login code | `routes/auth.rs`, `email.rs` | M3 |
| S3 | `POST /api/library/plays/batch [{track_id, seconds, played_at}]` (cap 500; scrobble with the given timestamp) | `routes/library.rs`, `scrobble.rs` | M5 |
| S4 | `GET /api/library/tracks?ids=1,2,3` (max 300) | `routes/library.rs` | M1 (queue restore, remote queues; falls back to per-id) |
| S5 | `GET /api/library/search?q=&limit=` returning `{tracks, albums, artists}` with totals | `routes/library.rs` | M3 |
| S6 | Stream client capability: `accept=flac,alac,opus,aac,mp3,wav` on `/stream`. The server transcodes only what isn't accepted (replaces the browser list for app clients) | `routes/library.rs` (StreamQ), `dsp.rs` | M2 |
| S7 | Artists paging (`limit/offset/total`); `total` on `/tracks` | `routes/library.rs` | M3 |
| S8 | Share `url` always absolute (fallback to request Host); `GET /api/shares`, `DELETE /api/shares/{token}` | `routes/library.rs` | M2 |
| S9 | `GET /api/app/bootstrap` → `{info, me, prefs, dsp, likes, playlists(summary), recent}`, one round trip on cold start | new `routes/app.rs` | M2 |
| S10 | Playback prefs schema/defaults served from `defaults.json` (`GET /api/auth/prefs/schema`), shared by web and app | `routes/auth.rs`, `web/js/playback.js` | M3 |
| S11 | Per-user notifications: `/api/notifications` feed + SSE event `request` on status change; later push (ntfy topic per user or UnifiedPush endpoint registration) | `notify.rs`, `routes/requests.rs` | after M5 |
| S12 | `api: <int>` in `/api/auth/info` | `routes/auth.rs` | M1 |
| S13 | `ETag` on `/api/library/albums`, `/artists`, `/playlists` (hash of max(mtime/updated_at)+count) → 304s; later `/api/library/changes?since=` with tombstones | `routes/library.rs`, `playlists.rs` | M3 (ETag), later (delta) |

Each change is additive and old web clients keep working. The app checks `features`/`api` and falls back where noted.

---

## 13. Milestones (each shippable on its own)

**M1: Sign in, library, native playback, Android Auto parity.** Replaces the Tauri APK.
- Server picker, sign-in + TOTP (bearer if `app_tokens`, else cookie), sign out.
- Home, Library (albums, artists, songs), Album, Artist (library), Playlists (read) + Liked, like/unlike, basic library search.
- `EncorePlaybackService` with a single ExoPlayer: original/transcode streams, gapless, queue, mini + full player, notification/lock screen, resume, scrobbling, autoplay.
- Android Auto: same browse tree, search and resumption as `CarService` today, with content:// artwork.
- Theme (Encore/OLED/Light). CI job, real signing.
- *Acceptance:*
  - (a) Installing over the last Tauri APK (same key) upgrades and keeps Auto permission.
  - (b) An album plays gapless (test album with continuous tracks; no audible gap, and the `ExoPlayer` analytics gap is under 5 ms).
  - (c) A locked phone plays for 2 h without stopping.
  - (d) The DHU browses all root folders, voice "play <album>" works, resume after reboot works.
  - (e) A scrobble appears in Last.fm/ListenBrainz with the same rule as web.
  - (f) A cold start with a warm DB shows Home in under 1 s.
  - (g) Server log: no requests while paused in the background.

**M2: Playback parity and Cadenza on the phone (effects).**
- Quality modes + adaptive ladder + mid-track step-down, preload toggle, server-synced `playback` prefs, S6/S9 when present.
- `cadenza` module + JNI; EQ/presets/preamp, crossfeed, balance, mono through `CadenzaAudioProcessor`.
- Signal path sheet. Lyrics (synced). Share links (+ system share sheet). Share-to-Encore target posting `/api/requests/link`.
- *Acceptance:*
  - (a) Throttling the network to 200 kbps steps down to 160→96 without stopping, then back up at the next track on Wi-Fi.
  - (b) Each EQ preset matches the web preset within 0.1 dB (unit test on the frequency response against playback.js fixtures).
  - (c) Effects off means `CadenzaAudioProcessor.isActive=false` (verified in the signal path).
  - (d) Under 5 % CPU for EQ+crossfeed at 96 kHz on a mid-range test phone.
  - (e) Changing prefs on the web shows up in the app after the next foreground.

**M3: Discover, For you, Requests, playlist editing, Account.**
- Search with MusicBrainz, artist/album catalog pages, For you/Made for you/new releases/tags.
- Requests list + new requests + picker (perm). Playlist create/rename/reorder/remove/import/request-missing. Leaderboard.
- Account: profile, avatar, password, 2FA, sessions with revoke (S1 fields), email-code login (S2).
- *Acceptance:* every listener-facing web route in section 8 has a native screen; a request made in the app appears on the web within one refresh; revoking the app session on the web signs the app out on its next call.

**M4: Remote devices and crossfade.**
- Phone as a device (SSE + throttled heartbeat), Devices sheet, control other devices, transfer playback. `CrossfadePlayer` with keep-albums-gapless.
- *Acceptance:*
  - (a) The web player controls the phone (all 11 actions), and the phone controls a browser tab.
  - (b) The phone appears within 2 s and disappears 5 min after a background pause.
  - (c) A 6 s crossfade between singles is audible, and none happens inside an album when keep-gapless is on.
  - (d) Heartbeat rate is ≤ 1 per 10 s while SSE is up (server log).

**M5: Offline.**
- Download track/album/playlist (Original or Compact), Downloaded tab, storage management.
- Offline playback in app and Auto (`isOffline` root), offline play log + S3 batch upload, pending mutations sync.
- *Acceptance:* airplane mode plays downloaded albums gapless in app and Auto; plays made offline reach the server with their real times after reconnect (with S3); a 2 GB download resumes after a process kill.

**M6: Hi-res output.**
- Phone Cadenza upsampling (filter/dither/rate catalog from `nativeCatalog`), bit-perfect USB via `AudioMixerAttributes` (API 34+), server Cadenza opt-in (admin-gated), `engine{set}` remote command.
- *Acceptance:*
  - (a) On a phone that offers BIT_PERFECT, a 24/96 FLAC to a USB DAC shows "Bit-perfect · 96 kHz/24" in the signal path, and the DAC's own display reports 96 kHz.
  - (b) A Cadenza 44.1→176.4 kHz upsample reaches the DAC at 176.4 kHz.
  - (c) Unplugging the DAC pauses and restores the mixer.
  - (d) With server Cadenza off for the user, no `dsp=` requests are made.

**M7: Clean-up.** Remove the Android target from `clients/app` (the `plugins/media` Android code and the `android` CI job). Remove the `tauri-plugin-encore-media` mobile dependency and the mobile parts of `bridge.rs`, `mobile.rs` and `offline.rs`. Update `README.md` and `docs/`.

---

## 14. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Past APKs signed with throw-away CI debug keys | No in-place upgrade; users reinstall once | Decide D1 now; from M1 on, releases fail without the real key |
| Bit-perfect not offered by many OEM HALs | M6 feature unavailable on some phones | Detect via `getSupportedMixerAttributes`; honest signal path; normal mixer at native rate as fallback |
| Long FIR CPU/thermal on mid-range phones | Dropouts, battery drain | `process_into` without allocations, filter tiers by device class, pre-designed coefficients cached on disk, warn on thermal status (`PowerManager.getThermalHeadroom`) |
| Crossfade via two players | Complexity, focus/scrobble glitches | Isolated behind `EncorePlayer`; ships in M4 after the single-player path is proven |
| Transcoded pipes (no Range/length) | Seeks re-open with `t=`, no gapless across lossy tracks, server ffmpeg load | Prefer originals; S6 `accept=` reduces transcodes; rung locked within an album; Compact offline only on request |
| Doze kills SSE/heartbeats when paused | Phone can't be woken remotely | Accepted for now; push via S11 later |
| Android Auto hides sideloaded apps | Auto unusable without developer setting | Document the toggle; Play listing (D3) fixes it |
| Media3 1.11 API details unverified here (float output builder names, `AudioOutputProvider` shape, processor latency hook) | Rework at M2/M6 | Spike each in the milestone's first week; pin Media3 1.11.1 |
| cadenza repo access from CI | Build fails | Public repo or deploy key (D2); vendoring as a git submodule as plan B |
