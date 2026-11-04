# Android performance

How smooth the Android app is, how it is measured, and what changed to make it smoother. The goal: lists that
scroll without stutter, players that don't redraw more than they need to, and a fast start, with no change to how
the app looks or what it does.

## How it is measured

`:benchmark` (Macrobenchmark) drives the release build (`benchmarkRelease`: R8 on, not debuggable, signed with the
debug key) through six journeys, written once in `journeys/Journeys.kt` and shared with the Baseline Profile
generator:

| Journey | What it does | Metric |
|---|---|---|
| `coldStartup` | starts the app with no process running | time to initial display |
| `warmStartup` | brings back an app that was sent to the background | time to initial display |
| `scrollLibraryAlbums` | Library → Albums, flings the grid from top to bottom | frame timing |
| `openAlbumAndScrollTracks` | opens the first album and flings its page to the end | frame timing |
| `fullPlayerPlaying` | opens the full player while a song plays and watches it for 10 s | frame timing |
| `upNextQueue` | opens Up next, scrolls it and removes a queued song | frame timing |

Frame timing reports the frame duration (CPU time per frame) at P50, P90 and P99, and janky frames: frames whose
overrun is above 0 ms, that is frames that missed their display deadline. Each frame journey runs 5 iterations,
each startup 10; every iteration starts from a stopped app.

The phone's Up next has no drag to reorder (only group listening has ↑/↓ buttons), so `upNextQueue` removes a song
instead: every song after it moves up, which is the reorder the list animates.

### Run it

On a phone with USB debugging (Android 9+; 13+ for the Baseline Profile without root), with the Android SDK and
Gradle 9.6.1, against any Encore server with a few albums:

```sh
cd clients/android
ENCORE_SERVER=https://music.example.com ENCORE_USER=me ENCORE_PASSWORD=… \
  COMPILATION=partial LABEL=after bash benchmark/ci/run.sh
```

`COMPILATION=partial` measures the app as a fresh install with its Baseline Profile; `COMPILATION=none` measures it
with no ahead-of-time compilation (the app before it shipped a profile). The tables are printed and saved to
`benchmark/build/summary.md`; Macrobenchmark's JSON and Perfetto traces are in
`benchmark/build/outputs/connected_android_test_additional_output/`. `STRICTMODE=1` adds a pass of the debug build
through the same journeys and summarises StrictMode's main-thread disk and network violations and JankStats' janky
frames per screen.

In CI, `.github/workflows/android-perf.yml` does the same on an Android 34 emulator against a server in Docker with
a made-up library (80 albums of 10 songs, 1200 px covers: `benchmark/ci/make-library.sh`), on every push to a
`claude/**` or `perf/**` branch that touches the app, or by hand from the Actions tab.

### Emulator caveats

The CI numbers come from a software-rendered emulator (swiftshader) on a shared runner. Every frame there misses
its deadline (janky share 100 % before and after), and the same commit varies by tens of percent between runs (album
page P90: 97 ms, 114 ms and 355 ms on three runs of unchanged app code). Use them to compare large differences and
to catch regressions in startup; judge smoothness by a run on a real phone.

## Results

CI emulator (Pixel 6 profile, Android 34, x86_64, software rendering), release build. *Before*: `d9762ee`, the
app as it was, with no ahead-of-time compilation (`CompilationMode.None`, as an app without a profile runs after
install). *After*: `4b13db5`, every change below, with its Baseline Profile installed (`CompilationMode.Partial`).
A run on a real phone is still to come (see "Run it").

### Startup (time to initial display, median of 10 / min / max)

| Journey | Before | After |
|---|---|---|
| coldStartup | 1051.8 ms (934.5 / 1184.8) | 897.3 ms (713.1 / 1106.3) |
| warmStartup | 859.9 ms (750.1 / 1074.1) | 500.9 ms (308.6 / 688.8) |

### Frame duration (CPU time per frame: P50 / P90 / P99, 5 iterations)

| Journey | Before | After |
|---|---|---|
| scrollLibraryAlbums | 90.4 / 116.4 / 142.1 ms | 76.3 / 90.3 / 103.9 ms |
| openAlbumAndScrollTracks | 75.7 / 336.6 / 451.8 ms | 61.1 / 125.9 / 272.5 ms |
| fullPlayerPlaying | 108.8 / 272.3 / 916.7 ms | 125.5 / 147.4 / 390.4 ms |
| upNextQueue | 173.0 / 376.3 / 1243.8 ms | 163.0 / 218.4 / 615.9 ms |

### Frames drawn per iteration (janky share)

| Journey | Before | After |
|---|---|---|
| scrollLibraryAlbums | 52.0 (100 %) | 69.2 (100 %) |
| openAlbumAndScrollTracks | 18.8 (100 %) | 22.8 (97 %) |
| fullPlayerPlaying | 45.4 (100 %) | 198.8 (100 %) |
| upNextQueue | 56.4 (100 %) | 145.6 (100 %) |

What the emulator shows:

- **Shorter long frames everywhere.** P90 fell by 22–62 % and P99 by 27–57 % on every frame journey: fewer of the
  stalls a person notices while scrolling, opening an album, watching the player or editing Up next.
- **Faster start**: warm start 860 → 501 ms, cold start 1052 → 897 ms (medians).
- **More frames where something now moves.** The full player draws about four times as many frames because the
  progress bar now moves every frame instead of twice a second, and Up next more than twice as many because removed
  rows slide away and the rows below slide up instead of jumping. More frames in the same time on the album grid
  means the fling ran more smoothly, not longer.
- **Every frame misses its deadline on this emulator**, before and after: it renders on the CPU, so the janky share
  says nothing here. That is the number a phone run will show.

How much to trust it: two intermediate runs (`84ae0e7`, the code changes without the profile, and `b83c581`, with
the profile but before the progress bars got their own layer) landed between the two columns, and four runs of the
unchanged app spread by up to 2× on a single P90. The direction is consistent across runs; the size of each gain
needs the phone run.

## What changed

Each change is under the hood: nothing looks or behaves differently.

1. **Baseline Profile and startup profile** (`:baselineprofile`, `app/src/main/generated/baselineProfiles/`). The
   generator records the startup and the journeys above; the release APK ships the profile (`assets/dexopt/`) and
   `androidx.profileinstaller` installs it on first launch, so ART compiles the startup, list and player code ahead
   of time instead of interpreting it while you scroll. The startup profile orders the DEX files so a launch reads
   the classes it needs together. `apps.yml` fails a build whose profile is missing, empty, has no rules for
   Encore's classes, or whose APK lacks it. *Effect: warm start and the first scrolls (see the tables).*
2. **Players stop recomposing twice a second** (`ui/screens/PlayerScreens.kt`). The mini and full player used to
   collect the position at their top, so the cover, title, buttons and menus recomposed on every half-second tick.
   Now the position is read only where it is shown: the progress lines read it in the draw phase (`drawBehind`, in
   their own layer), the seek bar through its `SliderState` (a `snapshotFlow` outside composition), the elapsed time
   through a `derivedStateOf` that changes once a second, and the synced lyrics through one that changes once a
   line (`LyricsScreen.kt`). *Effect: no recomposition of the player's content while a song plays.*
3. **Smooth progress** (`ui/PlaybackClock.kt`, `ui/PlayerConnection.kt`). The 500 ms tick stays; each tick and
   player event now also publishes a `PlaybackClock` (position, when it was read, playing, speed), and
   `rememberLivePosition` works out the position of each frame with `withFrameNanos` while the song plays and the
   player is on screen. It stops when paused, when the mini player is covered by the full player, and when the app
   is in the background (Compose stops the frame clock). *Effect: the bar moves every frame instead of jumping
   every half second. More frames are drawn while the player is open, which the emulator's software rendering
   shows as more frames per iteration in `fullPlayerPlaying`; each is a redraw of one thin layer.*
4. **Stable keys and content types** (`ui/components/ListKeys.kt`). Up next rows get a key per queue entry, not
   per song, because the same song can be queued twice; `ListKeys.carry` keeps each entry's key across removals,
   moves and additions, and the rows animate with `animateItem()` (phone and group Up next). Home's rails, the
   artist catalogue's rails and songs, the playlist picker and the playlist songs are keyed. The paged Library
   lists give placeholders and loaded rows their own content types, so Compose reuses a row's layout for the same
   kind of row. *Effect: a removal or move in Up next recomposes only the moved rows, and they slide into place.*
5. **Composition work moved out of recomposition**. Album pages (disc grouping, formats), artist catalogue
   (grouping, owned and local-only sets) and playlist pages (covers, sorting, filtering) recomputed lists on every
   recomposition; they are now `remember`ed on their inputs.
6. **Checked, nothing to change**: every cover (`AsyncImage`, `Cover`) fills a sized box, so Coil decodes it at
   the box's size, never at full size. The loader settings, `CoverPreloader`, 400 px covers, R8 and strong
   skipping are as they were.
7. **Preferences and the Cadenza library load in the background at start** (`core/Warmup.kt`). StrictMode showed
   the main thread waiting for SharedPreferences files and `libcadenza.so` the first time a screen or the playback
   service read them (Library's sort, the offline store, TV and car settings, the remote device, the server route,
   Cadenza's availability). A background thread now opens every preferences file and the library first thing at
   start, so those reads find them in memory. *Effect: no disk wait on the main thread when Library, the player or
   a TV panel opens for the first time.*
8. **Debug-only checks** (`core/DevPerf.kt`): StrictMode (main-thread disk reads and writes, network, logged) and
   JankStats (janky frames logged with the current screen: Home, Library, an album, FullPlayer, UpNext, …) in
   debug builds; release builds turn neither on.

## Debug-build checks

A debug-build pass through the same journeys (`STRICTMODE=1`) logs StrictMode's main-thread violations; there was
no main-thread network access, only disk reads (and two first writes that create a folder).

| Where | What it reads | Done |
|---|---|---|
| `CadenzaEngine.available` (15 before, 0 after) | loads `libcadenza.so` | loaded by `Warmup` first: gone |
| `LibraryScreen`, `LibrarySorts` (6 before, 0 after) | Library's preferences files | loaded by `Warmup` first: gone |
| `OfflineStore`, `CarPrefs`, `TvPrefs`, `RemoteDevice`, `ServerRoute`, `PrefsRepository`, `GroupListen`, `AuthScreens.legacyServer` | a preferences file's first read | `Warmup` loads them too, but these are read in the first milliseconds after start (the graph, the playback service, the first frame), usually before the background thread gets to them; still logged, and a later read never waits |
| `CrashLog.install`, `Translations.init`, `EncoreApp.onCreate` | the crash folder, the translation setting | kept: needed before the first frame, a few hundred bytes, once per start |
| `buildHttpClient`, `ApiCache.init`, `EncorePlaybackService.onCreate`, `CoverPreloader` | `cacheDir`/`filesDir` (a folder check) | kept: a single `stat` each, once per process |

Moving the remaining start-up reads off the main thread would mean building the app's graph and the playback
service asynchronously, which changes the order things start in; that is a larger change than this pass.

JankStats (debug build, so slower than release; compare screens with each other): UpNext 12 janky frames (median
12 ms, the slide animations), Library 6 (131 ms, the first page and covers arriving), Home 4 (291 ms, the first
frame after start), Album 4 (68 ms), FullPlayer 3 (137 ms, opening it).

## Lint

`android-perf.yml` runs Android lint and prints the issues in the files this branch changed. Fixed: a `StateFlow`
read during composition in the player's menu (now `collectAsState`) and every `mutableStateOf(0)` in the changed
screens (now `mutableIntStateOf`, no boxing). Left as they were, because they predate this work and fixing them
would change behaviour or dependencies rather than performance: the manifest notes (media search intent filter,
exported services, backup rules), newer library and SDK versions, `Configuration.screenWidthDp` in two layouts
(its replacement measures the window differently), and two `SharedPreferences.edit` KTX suggestions (the app has
no `core-ktx` dependency).
