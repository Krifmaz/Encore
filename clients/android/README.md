# Encore for Android (native)

The native Android client: Kotlin, Jetpack Compose (Material 3) and Media3. It replaces the Tauri APK
from `clients/app` and keeps its `applicationId` (`app.encore.client`), so it installs over it as an
update when the APK is signed with the same key. The plan is in
[`docs/android-native/PLAN.md`](../../docs/android-native/PLAN.md). This tree is **milestone M1**:

- server picker, sign-in with an authenticator code (2FA), sign out
- Home, Library (albums, artists, songs), Album, Artist, Playlists with Liked Songs, library search, like and unlike
- native playback in one `MediaLibraryService` (ExoPlayer). It covers the queue, the mini and full player, gapless
  playback, the notification and lock screen, resume (also after a reboot), "then shuffle library" (autoplay), and
  the play log and scrobbling
- Android Auto: the same browse tree, search, voice play and resumption as the old `CarService`, with `content://` artwork
- themes: Encore (charcoal and cream), OLED and Light
- "Share → Request in Encore": a link shared from another app is requested on the server

## Building

You need JDK 17 or newer (CI uses Temurin 21) and an Android SDK with `platforms;android-36` and
`build-tools;36.0.0`. Point `ANDROID_HOME` at the SDK, or put `sdk.dir=…` in `clients/android/local.properties`.

**No Gradle wrapper JAR is committed** because the repository holds no binaries. Only
`gradle/wrapper/gradle-wrapper.properties` is here, and it pins Gradle **9.6.1** (AGP 9.4 needs 9.6.0 or newer).
To build, do one of these:

- Use an installed Gradle 9.6.1: `gradle -p clients/android :app:assembleDebug`
- Generate the wrapper locally once with `gradle -p clients/android wrapper --gradle-version 9.6.1`. Then run
  `./gradlew …` and don't commit `gradle-wrapper.jar`.
- In CI, use `gradle/actions/setup-gradle@v4` with `gradle-version: 9.6.1`. It puts `gradle` on the `PATH` and
  caches dependencies:

```yaml
  android-native:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-java@v4
        with: { distribution: temurin, java-version: '21' }
      - uses: gradle/actions/setup-gradle@v4
        with: { gradle-version: '9.6.1' }
      - uses: android-actions/setup-android@v3
        with: { packages: 'platform-tools platforms;android-36 build-tools;36.0.0' }
      - name: Release keystore (fails when a secret is missing)
        env:
          ANDROID_KEYSTORE_BASE64: ${{ secrets.ANDROID_KEYSTORE_BASE64 }}
          ANDROID_KEYSTORE_PASSWORD: ${{ secrets.ANDROID_KEYSTORE_PASSWORD }}
          ANDROID_KEY_ALIAS: ${{ secrets.ANDROID_KEY_ALIAS }}
          ANDROID_KEY_PASSWORD: ${{ secrets.ANDROID_KEY_PASSWORD }}
        run: |
          for s in ANDROID_KEYSTORE_BASE64 ANDROID_KEYSTORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD; do
            [ -n "${!s}" ] || { echo "::error::secret $s is not set"; exit 1; }
          done
          echo "$ANDROID_KEYSTORE_BASE64" | base64 -d > "$RUNNER_TEMP/release.jks"
          echo "ANDROID_KEYSTORE_PATH=$RUNNER_TEMP/release.jks" >> "$GITHUB_ENV"
      - name: Build
        env:
          ANDROID_KEYSTORE_PASSWORD: ${{ secrets.ANDROID_KEYSTORE_PASSWORD }}
          ANDROID_KEY_ALIAS: ${{ secrets.ANDROID_KEY_ALIAS }}
          ANDROID_KEY_PASSWORD: ${{ secrets.ANDROID_KEY_PASSWORD }}
        run: |
          gradle -p clients/android :app:assembleRelease \
            -PencoreVersionName=${{ inputs.semver || '0.0.1' }} -PencoreVersionCode=${{ inputs.code || github.run_number }}
          cp clients/android/app/build/outputs/apk/release/app-release.apk "Encore-android-${{ inputs.semver || '0.0.1' }}.apk"
```

### Versions and signing

- `-PencoreVersionName=1.4.0 -PencoreVersionCode=26101300` set `versionName` and `versionCode`. The code uses the
  YYMMDDNN scheme from `scripts/release-apps.sh`, so it is always higher than the last Tauri build. Without them the
  build is `0.0.1-dev` with code 1.
- The release build reads the keystore from the environment: `ANDROID_KEYSTORE_PATH` (a file; CI decodes
  `ANDROID_KEYSTORE_BASE64` into it), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD`.
  **If `ANDROID_KEYSTORE_PATH` is missing, the release build is signed with the debug key.** That is handy
  locally. The release pipeline must refuse it, as the snippet above does.
- Debug builds get the suffix `.debug` (`app.encore.client.debug`), so they can be installed next to the real app.
- R8 (minify and resource shrinking) is on for release. `app/proguard-rules.pro` keeps the kotlinx.serialization
  serializers and JNI methods (for Cadenza in M2).

## Code layout

There is one Gradle module, `:app`, with packages under `app.encore.android`. The plan's multi-module split (core,
feature, playback, cadenza…) comes when the code grows. Until then the packages follow the same lines:

| Package | What |
|---|---|
| `data.api` | `EncoreApi` (OkHttp plus kotlinx.serialization), `AuthInterceptor`, models. Responses are read leniently from `JsonObject`, because SQLite rows have dynamic types; request bodies are `@Serializable`. |
| `data.session` | `SessionStore` (DataStore Preferences) holds the server, token and auth kind, the device id, and the theme and autoplay settings |
| `data.library` | `LikesRepository` (optimistic like and unlike) |
| `playback` | `EncorePlaybackService` (MediaLibraryService plus ExoPlayer), `BrowseTree` (Android Auto), `MediaItems`, `ResumeStore`, `CoverProvider` |
| `ui` | `MainActivity`, `ShareActivity`, `PlayerConnection` (MediaController), `EncoreRoot` (tabs and back stack) |
| `ui.screens` | Server and sign-in, Home, Library, Album, Artist, Playlists, Search, Settings, mini and full player |
| `ui.theme` | Colours from `web/css/app.css` (Encore, OLED, Light); the cream accent `#f0ead6 → #d6c49a` |

One `OkHttpClient` serves API calls, cover images (Coil) and audio streams (Media3 `OkHttpDataSource`). They all
share the auth header, the User-Agent `EncoreAndroid/<version> (Android <release>; <model>)` and the connection
pool. The session is only sent to the signed-in server, never to other hosts.

## How it talks to the server

**Sign-in** (`server/src/routes/auth.rs`):

- `GET /api/auth/info`: when `features` contains `app_tokens` (server change S1), the login body adds
  `app: {device_id, device_name}`. The reply is `{user, token}`, and every request then sends
  `Authorization: Bearer <token>`.
- Otherwise (current servers) the app keeps the `encore_session` cookie from `Set-Cookie`. It sends
  `Cookie: encore_session=…`, plus `X-Requested-With: encore` on every non-GET request (the server's CSRF check).
- A `401 {two_factor: true}` reply shows the code field.
- A `401` on any signed-in call signs the app out.

**Streams** (`server/src/routes/library.rs`, `StreamQ` / `browser_type`): the app requests
`/api/library/tracks/{id}/stream`.

- FLAC, MP3, AAC/M4A, Ogg/Opus, WAV and WebM come as the original file (Range-capable, gapless, no ffmpeg), and
  ExoPlayer decodes them natively.
- Every other format (DSF/DFF, APE, WavPack, WMA, AIFF, TTA, MPC …) gets `?transcode=1` and arrives as FLAC.
- ALAC inside `.m4a` is converted by the server on its own.

**Play log**, the same rules and endpoints as `web/js/player.js`:

- `POST /api/library/now-playing {track_id}` when a song starts. This feeds the Last.fm and ListenBrainz "now
  playing" status.
- `POST /api/library/plays {track_id, seconds}` once a song reaches half its length or 4 minutes, whichever comes
  first. Songs under 30 s count at their end.
- The check runs once a second and only while playing, so no requests are made while paused.

**Autoplay**: when the queue reaches its last song, `POST /api/library/shuffle {limit: 50, exclude}` appends a
shuffle of the library. It never repeats songs heard in this session. It can be turned off in Settings ("Then
shuffle library").

**Share target**: `ACTION_SEND text/plain` takes the first link in the text and sends it to
`POST /api/requests/link {url}`. The result appears as a toast.

## Android Auto

- `EncorePlaybackService` declares both `androidx.media3.session.MediaLibraryService` and
  `android.media.browse.MediaBrowserService`, plus `res/xml/automotive_app_desc.xml`.
- The browse tree has three tabs:
  - **Home**: recently added, recently played, made for you, shuffle everything
  - **Playlists**: Liked Songs first
  - **Library**: albums, artists
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
| com.squareup.okhttp3:okhttp | 5.5.0 |
| org.jetbrains.kotlinx:kotlinx-serialization-json | 1.11.0 |
| org.jetbrains.kotlinx:kotlinx-coroutines-android | 1.11.0 |
| io.coil-kt.coil3: coil-compose, coil-network-okhttp | 3.6.3 |
| SDK | minSdk 26, targetSdk 36, compileSdk 36 |

These choose different versions from the plan:

- **Compose BOM**: the plan names 2026.09.00, which brings Compose 1.12. Compose 1.12.0 raised its required
  `compileSdk` to 37 ("Updated Compose compileSdk to API 37", 1.12.0-alpha01), and AGP 9 refuses libraries that
  need a higher compileSdk than the app. To keep compile and target SDK at 36 as agreed, M1 uses BOM 2026.06.01. It
  has the same material3 1.4.0, with Compose 1.11.4.
- **Moving to BOM 2026.09.00** means setting `compileSdk = 37` (targetSdk can stay 36) and installing
  `platforms;android-37` in CI.
- **activity-compose and DataStore**: the plan left these open ("pin at M1"). They are the current stable releases.

M1 does not use Hilt, Room, Retrofit, navigation-compose, WorkManager or Tink yet. Navigation is a small typed
back stack in `EncoreRoot`, and data is loaded per screen. These come with the milestones that need them.

## Known limits in M1

- The session token is stored in plain DataStore (app-private, and `allowBackup=false`). The Tink/Keystore
  encryption from PLAN §4.2 only replaces `SessionStore`.
- Transcoded streams (DSD, APE …) are a pipe with no Range, so seeking in them doesn't work yet. Re-opening with
  `t=` comes with the quality ladder in M2. Originals seek normally.
- There are no quality modes or data saver yet (M2), no remote devices (M4; the old car device id `car-…` is gone),
  and no offline downloads (M5).
- Android Auto doesn't have per-item Like or Play next buttons yet (PLAN §10).
- The Tauri app's offline files are not cleaned up on first run. The server it saved is offered as a suggestion on
  the server screen.
- The code was written without an Android SDK at hand. It was type-checked against the Media3 1.11.1 sources and
  Compose (desktop) APIs, but the first real build is the CI build.
