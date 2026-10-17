# Rewrite + backlog (collected Oct 16–17, 2026; plan due Tue Oct 20, 9 AM ET)

Nothing here is started until the owner says "go".

## The rewrite
- Full organized rewrite of Encore: server, web, native Android app (clients/android), desktop app (clients/app).
  The app looks and behaves exactly as now. Remove bloat, optimize everything.
- Drop the JS web UI (web/js). Keep only the Rust/WASM UI (web-rs). When WebAssembly is blocked (Vanadium JIT
  off, iOS Lockdown Mode), show a message explaining how to allow it for the site. The owner uses Brave.
- The phone app gets the FULL Settings, matching the web: every tab, including the admin ones.
- Cadenza:
  - Keep the engine built into the desktop and Android apps, but only as this device's own output path.
    Settings → Playback gets a "Play through Cadenza on this device" switch with output/DAC, exclusive mode and
    DSP settings.
  - Not a device in the device picker: remove "This computer (Cadenza)" as a device, along with its app-token
    and device registration.
  - It must actually reach the chosen output.
  - Keep and clean up server-side Cadenza processing.
  - The standalone Cadenza app (separate repo, not edited here) can still be a LAN device Encore controls, like
    Roon → HQPlayer.
- Finish the unfinished 0.25.3 work from WIP commit 2b5bdc2 (unverified):
  - YouTube sign-in through the extension hand-off, with the server keeping the login fresh; the server
    browser is removed.
  - Speed pass: "sluggish, takes forever to load" with about 57k tracks.
  - Artist names open the artist page.

## Bugs and features
1. **Car (Android Auto):** tapping the artist opens that artist's page.
2. **Lyrics mode (phone + web):** the album art shrinks into the play bar; lyrics get bigger text and more
   room.
3. **Volume normalization:** ReplayGain/loudness with an adjustable target level and track/album mode, on
   phone, web, car and desktop.
4. **Artist page:** a "Liked from this artist" section and an "In your library" section.
5. **Car audio quality** (FLAC 24/96, 320k…) on the now-playing screen, between Like and Next. Android Auto
   custom actions are icon-only, so use a badge icon there plus the full detail on the subtitle line.
6. **Liked Songs → Download missing → "Requested 0"** while 784 songs are missing.
   - Cause: `request_missing` (server/src/playlists.rs) skips rows that already have a request_id, even when
     that request failed, was not found or was deleted. 409 duplicates are skipped silently.
   - Fix: re-request finished-without-success or vanished requests, and link 409 duplicates to the existing
     request.
   - Report real counts ("Requested 120 · 600 already in progress · 64 couldn't be requested (why)").
   - Show per-row status on missing songs.
7. **Admin file manager (web + phone)** over the library and the import/download folders:
   - Browse, upload (folders too), move, rename, delete to trash, make folders, edit tags.
   - Manual import with match review, and a "keep the files where and as they are" option.
   - The database stays in sync. Paths are confined to the configured roots.
8. **Cadenza no audio on Windows.** Setup: RTX 3070; USB → Singxer SU-2 → Fosi ZD3 (ES9038Q2M), and the ZD3
   directly over USB.
   Seen on screen:
   - Exclusive is ticked, but the status says "wasapi · … · Shared".
   - DSD512 stayed selected on the ZD3, which supports DSD256 at most.
   - The error was `…/tracks/58742/stream: decode: unsupported feature: core (codec):unsupported codec` on an
     **Opus** file. The engine's decoder (symphonia) has no Opus support.

   a. Check the codecs the engine can decode. When it can't decode a file, the server sends a lossless FLAC
      transcode, shown in the signal path. Write a Cadenza-agent prompt to add Opus and check AAC, ALAC,
      WavPack, APE, DSF and DFF.
   b. Find why exclusive mode fails: the device held by another app, the Windows "allow exclusive" setting,
      or format negotiation/IsFormatSupported. Never fall back to shared silently; show
      "Exclusive mode failed: <reason>".
   c. Clamp the DSD/PCM rate options to the device's capabilities, and re-validate when the device changes.
   d. Block DSD/DoP in shared mode.
   e. A downstream link limit setting (USB / I2S / coax / optical; S/PDIF tops out at 192 kHz / DoP64).
   f. A "Test sound" button, detection of the app holding the device, clear errors and the full signal path.
   g. Check that the engine starts the stream and reports position (it sat at 0:00).
   h. List what needs the owner's hardware to confirm. Never edit the Cadenza repo; write a prompt for the
      Cadenza agent instead.
9. **Soulseek crash on 0.26.1:** "can't create /downloads/slskd: Permission denied".
   - Cause: docker/entrypoint.sh only creates and chowns the staging folder.
   - Fix: create and chown every configured working folder before dropping to PUID.
   - Check write access at start and on save, and show the exact chown command instead of crashing.
   - Fall back to a folder under /config.
   - After a crash, a settings change must start slskd at once (`Runner::apply` in server/src/slskd_managed.rs
     waits out the backoff first). Add a "Start now" button.
10. **Request discography (phone + web):**
    - Follow the server's Release types settings: pre-tick the allowed releases and show the others unticked.
    - Group by type with checkboxes, plus Select all / Deselect all for everything and per group.
    - Request only the ticked releases, with the count shown on the button.
11. **streamrip:** a request for "Reach Out To The Truth · 平田志穂子" failed with "streamrip produced no files: Error
    loading config from /config/streamrip/config.toml:".
    - Likely causes:
      - `apply_credentials` (server/src/sources/streamrip.rs) builds a partial config when streamrip didn't
        write its default (only [deezer]/[qobuz]/[database] sections).
      - Or the config is from an older streamrip version after a tool update.
    - Fix: validate the config after writing it (run streamrip's own config check). Regenerate from defaults
      (`config reset` / `--update`) when it's invalid or outdated, then re-apply the credentials.
    - Surface the full streamrip error (the text after "config.toml:" was empty).
    - Add a "Reset streamrip config" button.
12. **The download picker ("Pick a download") only offers Soulseek, Torrents & Usenet, Deezer and YouTube Music.**
    - Tidal and Qobuz are never listed, even when streamrip is signed in to them. `sources()` in
      server/src/routes/picker.rs only adds Deezer from streamrip's services.
    - Fix: add Tidal and Qobuz tabs: search their catalogs and show quality (e.g. FLAC 24/192 MQA/Hi-Res) per
      result.
    - Show every configured download source; a source that isn't signed in shows "Sign in to Tidal" instead
      of disappearing.
    - Seen while Soulseek was signed out ("409 … must be connected and logged in … (currently: Disconnecting)").
      Show that state as a short "Soulseek isn't connected yet" line with a link to its settings.
13. **streamrip asks for the Deezer ARL interactively** after a fresh config ("produced no files: … the ARL cookie
    automatically / 2. Copy the ARL cookie from your browser by hand"). The ARL had lived only in the old
    config.toml, not in Encore's `streamrip_deezer_arl` setting.
    - Fix: apply the credentials before every run, not only when the file is missing.
    - When rebuilding the config, carry over the credentials and tokens from the old file.
    - Run streamrip non-interactively (stdin closed); detect its prompts and show "Deezer needs an ARL cookie –
      Settings → streamrip" instead.
    - The Requests list piles up duplicate failed rows for the same song (4× "Reach Out To The Truth").
      Group retries of one song into one row.
14. **Offline website (PWA):** the site must open and play downloaded songs when the server is unreachable.
    - The service worker caches the whole app shell (the web-rs WASM, JS glue, CSS, fonts, icons) and serves
      it when the server is down.
    - "Download for offline" on the web: songs, albums, playlists and Liked Songs are saved in the browser's
      storage (OPFS/Cache API) together with their covers, lyrics and metadata. Call
      navigator.storage.persist(); show used/available space and let the user manage downloads.
    - An offline mode: when the server can't be reached, show the Downloaded library (search, play,
      queue, shuffle) and grey out everything else. Queue plays, likes and scrobbles and send them when back
      online.
    - Respect the user's session: downloads stay usable offline for a limited time (e.g. 30 days) without
      re-checking the login.
    - iOS Safari limits: storage is evicted for sites not added to the Home Screen. Tell iPhone users to "Add
      to Home Screen" for reliable offline use.
    - The phone and desktop apps already have offline songs; reuse the same model so they behave the same.
