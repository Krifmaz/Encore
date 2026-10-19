# Encore for Android (native)

The native Android client: Kotlin, Jetpack Compose (Material 3) and Media3, with Cadenza (the server's DSP engine)
inside the app through a small Rust library. It replaced the Tauri APK from `clients/app` and keeps its
`applicationId` (`app.encore.client`), so it installs over it as an update when the APK is signed with the same key.

What it does:

- server picker, sign-in with an authenticator code (2FA), app tokens, sign out
- tabs Home, Search, Library, Playlists and Discover; For you, Requests, the download picker, Leaderboard, Account
  and Settings; one artist page (MusicBrainz, or the library's albums for an artist known by name only)
- playback in one `MediaLibraryService` (ExoPlayer): queue, mini and full player, gapless, notification and lock
  screen, resume and crash recovery, "Keep going with a shuffle of the whole library", plays and now playing
- the web player's playback settings: quality Auto / Original / Data saver with the bandwidth estimate, the server's
  Cadenza (render ahead), sound effects (EQ, crossfeed, balance, mono), song transitions, the Signal path
- Cadenza on this phone (upsampling) and bit-perfect output to a USB DAC (Android 14+)
- this phone as an Encore remote device (play here, send there, control another device)
- Android Auto (browse, search, voice, live lyrics) and the Encore Lyrics car app
- "Share → Request in Encore", crash reports to the server (Diagnostics → App crashes)
- songs saved for offline in `Music/Encore`, offline mode, Downloaded in the Library tab and in Android Auto
- updates itself from the GitHub releases (Settings → This device → Updates)
- bug reports: the list, a report with its replies, a new report with pictures (Home menu and Settings → This
  device)

## Building

You need JDK 17 or newer (CI uses Temurin 21) and an Android SDK with `platforms;android-36` and
`build-tools;36.0.0`. Point `ANDROID_HOME` at the SDK, or put `sdk.dir=…` in `clients/android/local.properties`.
For Cadenza in the APK you also need Rust with the `aarch64-linux-android` and `armv7-linux-androideabi` targets,
`cargo install cargo-ndk --version 4.1.2 --locked` and an NDK (CI: `ndk;29.0.13846066`); without them the build
still works, but the APK has no Cadenza library: no sound effects, no phone upsampling (see `cadenza/README.md`).

**No Gradle wrapper JAR is committed** because the repository holds no binaries. Only
`gradle/wrapper/gradle-wrapper.properties` is here, and it pins Gradle **9.6.1** (AGP 9.4 needs 9.6.0 or newer).
To build, do one of these:

- Use an installed Gradle 9.6.1: `gradle -p clients/android :app:assembleDebug`
- Generate the wrapper locally once with `gradle -p clients/android wrapper --gradle-version 9.6.1`. Then run
  `./gradlew …` and don't commit `gradle-wrapper.jar`.

CI is the `android-native` job of `.github/workflows/apps.yml`: the JNI crate's host tests, `assembleRelease`, the
unit tests in `app/src/test` (`:app:testDebugUnitTest`, JUnit 4, plain Kotlin with no Android classes),
`apksigner verify`, the Cadenza library present for both ABIs, and the 16 KB page checks (every LOAD segment of a
`.so` aligned to 0x4000 by `llvm-readelf -l`, and `zipalign -c -P 16 -v 4` on the APK).

### Versions and signing

- `-PencoreVersionName=0.30.0 -PencoreVersionCode=260300000` set `versionName` and `versionCode`. The release
  workflow computes the code as 260000000 + major × 1000000 + minor × 10000 + patch × 10, so it always grows and
  stays above the old date-based codes (26101302 …); the in-app update refuses an APK whose code is not higher.
  Without them the build is `0.0.1-dev` with code 1.
- The release build reads the keystore from the environment: `ANDROID_KEYSTORE_PATH` (a file; CI decodes
  `ANDROID_KEYSTORE_BASE64` into it), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD`.
  **If `ANDROID_KEYSTORE_PATH` is missing, the release build is signed with the debug key.** That is handy
  locally; the release pipeline refuses it.
- Debug builds get the suffix `.debug` (`app.encore.client.debug`), so they can be installed next to the real app.
- R8 (minify and resource shrinking) is on for release. `app/proguard-rules.pro` keeps the kotlinx.serialization
  serializers and the JNI methods; `cadenza/consumer-rules.pro` keeps the JNI class.

### Updates

`features/update/` (Settings → This device → Updates). The phone reads
`https://api.github.com/repos/Krifmaz/Encore/releases/latest` itself (the ETag is kept, so an unchanged answer is a
free 304), at most once a day when the app opens and by hand. The file is the release asset
`Encore-android-<version>.apk` (apps.yml `android-native`); its version comes from the file name, because a release
made without the apps carries the last APK over. Only files under `github.com/Krifmaz/Encore/releases/download/` are
fetched (`UpdateRules.isReleaseFile` reads the URL as OkHttp does, so a `..` part can't lead elsewhere), and the APK
download stops past the size the release lists (300 MB when it lists none).

Install downloads the APK into `cacheDir/updates` and checks it before Android sees it: the SHA-256 against the
release's `SHA256SUMS` (and GitHub's `digest` of the asset) when the release has them, then the APK through
`PackageManager.getPackageArchiveInfo`: package `app.encore.client`, a higher `versionCode`, and the same signing
certificate as the installed app (`GET_SIGNING_CERTIFICATES` on Android 9+, a proven v3 key rotation included;
`GET_SIGNATURES` below). A file that fails is deleted. Without "Install unknown apps" for Encore, Android's page for it
opens first. Then a `PackageInstaller` session (`ApkInstaller`) with `USER_ACTION_NOT_REQUIRED` on Android 12+ (Android
may still ask); `UpdateInstallReceiver` gets the outcome and opens Android's confirm screen. Manifest:
`REQUEST_INSTALL_PACKAGES`, `UPDATE_PACKAGES_WITHOUT_USER_ACTION` and the receiver (not exported). Debug builds
(`.debug`) can't update themselves. The rules are plain Kotlin (`UpdateRules`) with JUnit tests in `app/src/test`.

### Offline songs

`features/offline/` (0.30, docs/contracts/offline-manifest.md). "Download for offline" in the song, album, playlist
and player menus saves the songs as files: `Music/Encore/<Album artist>/<Album>/NN - Title.ext`, the stream the
server sends, which is the file itself with its tags when the phone plays the codec (`X-Encore-Decoders:
flac,mp3,aac,opus,vorbis,wav` with `/info` and the stream; `offline_ok`), and FLAC the server makes otherwise, marked
`FLAC (converted)`. `OfflineWorker` fetches two at a time in the app's scope with a progress notification (no
foreground service, no WorkManager), then the album cover (600 px) and the lyrics.

- Android 10 and later: the audio goes through MediaStore Audio (`RELATIVE_PATH`, `IS_PENDING` while it is written;
  no permission for the app's own files). Android lets an app put only audio and playlists in `Music/`, so the index
  (`.encore-index.json`), the covers (`covers/<server>-<album>.jpg`) and the lyrics stay in the app's storage
  (`files/offline/`).
- Android 8 and 9: plain files with `WRITE_EXTERNAL_STORAGE` (asked at the first download; manifest
  `maxSdkVersion="28"`): the index is `Music/Encore/.encore-index.json` (a copy stays in the app's storage), the
  cover is `cover.jpg` in the album's folder; a reinstall picks the index up again.
- The index is the shared record of the manifest (`file` relative to `Music/Encore/`, `art` relative to the
  index's folder), one array for every server; lists and playback use the signed-in server's songs.
- A song with a copy plays from it (`StreamResolver.localCopy`, the `local` variant): no stream, no Auto steps or
  bandwidth probes, no server processing; the Signal path says "Downloaded copy on this device · FLAC 16/44.1". A
  copy that won't play streams from the server for the rest of the session. "Play downloaded songs from this
  device" (Settings → This device → Downloads, on by default, as the web's switch) turned off streams downloaded
  songs while the server can be reached; offline mode or no network always plays the copy.
- Offline mode (`OfflineMode`): the signed-in server doesn't answer `/api/health` (or a proxy says 502–504) and songs
  of it are saved here. The banner with Retry, the Library tab shows Downloaded, the other tabs are greyed; the
  server is asked again every 30 s, on network changes and on Retry. Plays and likes made meanwhile wait in the
  outbox (`files/offline/outbox.json`, owned by `<server>|<username>`) and go when it answers: the plays in the list
  form of `POST /api/library/plays`, the likes one by one ("Back online – 3 plays sent"). The hearts use the liked
  list kept from the last answer. A play or like the server could not take during a short gap (offline mode not
  on) waits there too and goes after the next play or like the server takes (`OfflineMode.syncSoon`, the web's
  `sync_outbox`). The playback service starts the network watch as well, so this works with only the car in use.
- Settings → This device → Downloads: the room they take, per-album remove, Remove all. Files deleted in a file
  manager leave the index at the next start; what a download cut off left behind goes at a later start (Android 10
  clears its unfinished MediaStore entries itself after a week).
- The rules (index, upgrade of older shapes, names, the quality label, the outbox) are plain Kotlin with JUnit tests
  in `app/src/test/java/app/encore/android/features/offline/`.

## Code layout

Two Gradle modules: `:app` (packages under `app.encore.android`) and `:cadenza` (the JNI binding `CadenzaNative` and
the Rust crate in `cadenza/rust`). No DI framework: `AppGraph` (`EncoreApp.kt`) builds the singletons by hand, and
navigation is a typed back stack in `ui/EncoreRoot.kt`. Comments that cite "PLAN §n" refer to the original plan, now
`docs/archive/android-native/PLAN.md`.

| Package | What |
|---|---|
| `core` | `Json.kt`: the one lenient JSON layer. `EncoreJson`, the `JsonObject` readers (`str`, `dbl`, `int`, `bool`, `obj`, `arr`, `objects` …) and the `L…` field types for `@Serializable` models (a missing, null or odd value becomes the empty value, `Items` lists skip a bad row, `Checked` leaves out rows without what they need). SQLite rows have dynamic types, so everything read from the network goes through it. |
| `data.api` | `EncoreApi` (OkHttp), `EncoreHttp` (one client, auth, User-Agent), library models (`Track`, `Album` …), `CoverVersions`, `Translations` |
| `data.repo` | `FeaturesApi` and `FeatureModels`: Discover, For you, Requests, picker, playlists, account, leaderboard, as `@Serializable` models |
| `data.prefs` | `PlaybackPrefs` (the web's playback settings, synced with the account), `PhonePrefs` (this phone's Cadenza and USB settings, device-only), `PrefsRepository`, `DspInfo` |
| `data.session`, `data.library` | `SessionStore` (DataStore: server, token, device id, theme), `LikesRepository` |
| `playback` | the service and the stream: see below |
| `playback.output` | `EffectsAudioProcessor` (+ `EffectsChain`), `OutputRouter` (phone Cadenza, USB routing), `UsbOutputManager`, `DeviceOutput` |
| `cadenza` | `CadenzaEngine` (the player's side of `CadenzaNative`: setup per stream, direct buffers), `CadenzaCatalog` (filters and dithers, the phone's and the server's) |
| `remote` | `RemoteDevice` (check-ins, SSE commands, linking), `ServiceHost` (the service's side), `DevicesSheet` |
| `features.offline` | songs saved on the phone: `OfflineStore` (files, MediaStore, the index), `OfflineWorker` (downloads), `OfflineMode` (offline mode, the outbox), `OfflineIndex` / `OfflineOutbox` (the plain rules), `OfflineUi` (menus, the Downloaded list, Settings → Downloads, the banner) |
| `features.update` | the phone updates itself: `UpdateRules` (the plain rules), `Updater` (GitHub, the download, the checks), `ApkInstaller` (signer check, `PackageInstaller` session, `UpdateInstallReceiver`), `UpdatesSection` (Settings) |
| `features.bugs` | bug reports: `BugRules` (the plain rules), `BugReportsApi`, `BugAttachments` (photo picker, the JPEG fallback), `BugReportsScreen`, `BugReportScreen`, `NewBugReportSheet`; the export is shared through `androidx.core.content.FileProvider` (`<applicationId>.files`, `cacheDir/shared/` only) |
| `car` | Encore Lyrics car app (`LyricsCarAppService`, `LyricsSurface`), the lyrics card (`LyricsCard`), `CarNowPlaying` and `LyricsCache` shared with `CarControls` |
| `ui`, `ui.screens`, `ui.components`, `ui.theme` | `MainActivity`, `ShareActivity`, `PlayerConnection` (MediaController), `EncoreRoot`; the screens; shared widgets; the palettes from the web's CSS |

One `OkHttpClient` serves API calls, cover images (Coil) and audio streams (Media3 `OkHttpDataSource`). They all
share the auth header, the User-Agent `EncoreAndroid/<version> (Android <release>; <model>)` and the connection
pool. The session is only sent to the signed-in server, never to other hosts.

### Playback

`EncorePlaybackService` is the wiring: ExoPlayer, the Media3 library session and its callback (browse tree, voice
search, the custom seek and car commands, playback resumption), the session's extras for the UI (`StreamInfo`, one
JSON extra) and the player events, handed to the parts that act on them:

| File | What |
|---|---|
| `StreamResolver.kt` | builds each stream URL when ExoPlayer opens it: original, `transcode=1`, the AAC rungs, or `dsp=1` for the server's Cadenza; `BandwidthEstimator` |
| `CurrentStream.kt` | the playing song in song time (a server-made stream starts at an offset) and reopening it at a spot on another variant |
| `Scrobbler.kt` | now playing and plays (half the song or 4 minutes; songs under 30 s at their end) |
| `AdaptiveQuality.kt` | Auto quality: stalls, a slow start, probes while idle, step down at once, up after 15 s of 1.5× headroom |
| `ServerCadenza.kt` | the server's Cadenza: render ahead, seeks into a finished render, falling back to the original |
| `Autoplay.kt` | a shuffle of the library when the queue ends |
| `Resume.kt` (+ `ResumeStore`) | the queue and spot on this phone, restored paused at start, and crash recovery |
| `Transitions.kt` | crossfade / overlap / smart overlap with a second ExoPlayer for the old song's tail (its own Cadenza engine, effects only) |
| `CarControls.kt`, `BrowseTree.kt`, `MediaItems.kt`, `CoverProvider.kt` | Android Auto and the session's buttons, the browse tree, media ids, `content://` covers and lyrics cards |

The sound path: ExoPlayer decodes to 16-bit PCM; `EffectsAudioProcessor` (the only processor in the sink) converts it
to floats straight into a direct buffer of `CadenzaEngine`; `CadenzaNative.nativeProcess` runs the effects (and,
when "Upsample on this phone with Cadenza" is on, the resampler) in place over that memory, or into a second
direct buffer when upsampling makes the output longer; the processor writes 16-bit with TPDF dither, or 24 / 32-bit
for Cadenza's output and bit-perfect USB. Effects off at the song's rate pass the bytes through untouched.

## How it talks to the server

**Sign-in** (`server/src/routes/auth.rs`):

- `GET /api/auth/info`: when `features` contains `app_tokens` (server change S1), the login body adds
  `app: {device_id, device_name}`. The reply is `{user, token}`, and every request then sends
  `Authorization: Bearer <token>`.
- Otherwise (older servers) the app keeps the `encore_session` cookie from `Set-Cookie`. It sends
  `Cookie: encore_session=…`, plus `X-Requested-With: encore` on every non-GET request (the server's CSRF check).
- A `401 {two_factor: true}` reply shows the code field.
- A `401` on any signed-in call signs the app out.

**Streams** (`server/src/routes/library.rs`, `StreamQ` / `browser_type`): the app requests
`/api/library/tracks/{id}/stream`.

- FLAC, MP3, AAC/M4A, Ogg/Opus, WAV and WebM come as the original file (Range-capable, gapless, no ffmpeg), and
  ExoPlayer decodes them natively.
- Every other format (DSF/DFF, APE, WavPack, WMA, AIFF, TTA, MPC …) gets `?transcode=1` and arrives as FLAC.
- ALAC inside `.m4a` is converted by the server on its own.
- Auto quality and Data saver ask for `?q=aac&kbps=N`, the server's Cadenza for `?dsp=1&rate=&filter=&dither=`; a
  stream the server makes as it goes restarts at `t=<seconds>` for a seek (`StreamResolver`).

**Play log**, the same rules and endpoints as `web/js/player.js`:

- `POST /api/library/now-playing {track_id}` when a song starts. This feeds the Last.fm and ListenBrainz "now
  playing" status.
- `POST /api/library/plays {track_id, seconds}` once a song reaches half its length or 4 minutes, whichever comes
  first. Songs under 30 s count at their end.
- The check runs once a second and only while playing, so no requests are made while paused.

**Autoplay**: when the queue reaches its last song, `POST /api/library/shuffle {limit: 50, exclude}` appends a
shuffle of the library. It never repeats songs heard in this session. It can be turned off in Settings → Playback
("Keep going with a shuffle of the whole library").

**Share target**: `ACTION_SEND text/plain` takes the first link in the text and sends it to
`POST /api/requests/link {url}`. The result appears as a toast.

## Android Auto

- `EncorePlaybackService` declares both `androidx.media3.session.MediaLibraryService` and
  `android.media.browse.MediaBrowserService`, plus `res/xml/automotive_app_desc.xml`.
- The browse tree has three tabs, and a fourth once songs are saved on the phone:
  - **Home**: recently added, recently played, made for you, shuffle everything
  - **Playlists**: Liked Songs first
  - **Library**: albums, artists
  - **Downloaded**: shuffle, all songs, the albums; read from the offline index, so it works without the server.
    A browser that asks for the root with `isOffline` gets this node as the root.
- Every node id of the old `CarService` (`recent`, `album:<id>`, `playlist:<id>`, `mfy:<kind>`, `track:<id>|<parent>`
  …) still resolves.
- Picking a song plays the list it was shown in.
- Voice requests ("play <album or artist>") search the library.
- Resume uses the last queue, which is stored on the phone.
- Artwork is `content://<applicationId>.covers/album/<id>?s=600`. Android Auto and System UI can't send our session,
  so `CoverProvider` fetches each cover once with the app's client and serves it from the cache.
- Auto hides apps that weren't installed from Play unless "Unknown sources" is on in Auto's developer settings
  (decision D3: GitHub releases only).
- Not signed in (or the session expired): `onGetLibraryRoot` still returns the root, because Media3 does not
  replicate an error from that call and Auto would only show "can't use this app right now". Browsing the root
  (`onGetChildren`) then returns `RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED` with the
  `EXTRAS_KEY_ERROR_RESOLUTION_ACTION_*_COMPAT` extras, so Auto shows "Sign in to Encore on your phone" with a
  button that opens the app.

### Lyrics in the car

- The Lyrics button (next to Repeat) turns live lyrics on and off; Settings → Playback → In the car picks the look:
  **Card** (default) or **Title line**. Both only change what the session reports (`CarPlayer.showMetadata`), so
  playback never rebuffers.
- Card: `LyricsCard` draws an 800×800 picture (blurred, darkened cover; two past lines faded above, the current
  line large in cream, the next three faded below) on its own thread at each line change. It goes out as
  `artworkData` and as `content://<applicationId>.covers/lyrics/<version>` (a new URI per card, served by
  `CoverProvider` from `cacheDir/car-lyrics`, newest six kept). Title and artist stay as they are. No synced
  lyrics: the normal cover.
- Title line: the current line as the title, "<title> · <artist>" below it.

### Encore Lyrics (second car app)

- `car.LyricsCarAppService`: an `androidx.car.app` template app, category POI (not navigation), with the
  `androidx.car.app.MAP_TEMPLATES` and `ACCESS_SURFACE` permissions; `minCarApiLevel` 7 (MapWithContentTemplate,
  car-app 1.7). `automotive_app_desc.xml` declares `media` and `template`.
- The map surface (`LyricsSurface`) draws the lyrics: smooth scroll, the current line enlarged in cream, the others
  fading with distance, over the cover's colour. At most 30 frames a second while something moves, a position
  check 10 times a second otherwise, nothing while the screen is stopped. A tap on a line seeks to it (surface
  clicks need car API 5); a drag scrolls by hand for 4 s.
- The content pane shows title, artist, album and cover; previous / play-pause / next are in the action strip
  (a pane takes only two actions).
- The song and position come from the playback service in the same process (`CarNowPlaying`, re-published every
  second while the screen is shown); commands go through a MediaController on the service's session, which also
  starts the service when Encore Lyrics opens first. Seeking uses the service's `SEEK` command (absolute position,
  also for server-made streams).

## Libraries (all pinned in `gradle/libs.versions.toml`)

| Library | Version |
|---|---|
| Android Gradle Plugin | 9.4.0 (built-in Kotlin; `org.jetbrains.kotlin.android` is not applied) |
| Gradle | 9.6.1 |
| Kotlin, Compose compiler plugin, serialization plugin | 2.4.21 |
| Compose BOM | 2026.06.01 (Compose 1.11.4, material3 1.4.0) |
| androidx.activity:activity-compose | 1.13.0 |
| androidx.media3: exoplayer, session, datasource-okhttp | 1.11.1 |
| androidx.datastore:datastore-preferences | 1.2.1 |
| androidx.car.app:app (Encore Lyrics) | 1.7.0 (car API level 7 for MapWithContentTemplate) |
| com.squareup.okhttp3:okhttp | 5.3.2 |
| org.jetbrains.kotlinx:kotlinx-serialization-json | 1.11.0 |
| org.jetbrains.kotlinx:kotlinx-coroutines-android | 1.11.0 |
| io.coil-kt.coil3: coil-compose, coil-network-okhttp, coil-network-cache-control | 3.5.0 |
| junit:junit (unit tests only, not in the APK) | 4.13.2 |
| Rust crate `encore-cadenza-jni` | `cadenza-dsp` at the same git rev as `server/Cargo.toml`, `jni` 0.21 |
| SDK | minSdk 26, targetSdk 36, compileSdk 36 |

Held back: Compose BOM 2026.09 (Compose 1.12), OkHttp 5.4+ and Coil 3.6 need compileSdk 37. Moving means
`compileSdk = 37` (targetSdk can stay 36) and `platforms;android-37` in CI.

No Hilt, Room, Retrofit, navigation-compose or WorkManager: the graph is built by hand, data is loaded per screen.

## Known limits

- The session token is stored in plain DataStore (app-private, and `allowBackup=false`).
- The phone predicts the rate the server's Cadenza sends from a 44.1 kHz source (it does not know the file's rate
  before it plays), so the quality badge and the Auto ladder can be low for a hi-res file under server processing.
- Without the Cadenza library in the APK (a build without cargo-ndk, or a phone whose processor the APK has no
  library for, such as x86) there are no sound effects: the Kotlin copy of the effects was removed in 0.30, Cadenza
  is the one engine. Settings → Playback then greys the effects and says so.
- Bit-perfect USB output depends on the phone's audio HAL (`getSupportedMixerAttributes`); it uses the first USB DAC
  that offers it.
- Android Auto doesn't have per-item Like or Play next buttons.
- Downloads: on Android 10 and later the index and the covers are in the app's storage (Android keeps only audio in
  `Music/`), so after a reinstall the files in `Music/Encore` are ordinary music files and the app starts with no
  downloads. Server processing applies to a downloaded song only with "Play downloaded songs from this device"
  off. There is no 30-day limit offline (the web has one for its remembered sign-in).
- The code is written without an Android SDK at hand here: it is type-checked against the Media3, car-app and
  Compose APIs, and the real build is the CI build.
