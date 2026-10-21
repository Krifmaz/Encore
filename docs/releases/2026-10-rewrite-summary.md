## The October 2026 rewrite, 0.27 to 0.33, on one page

The plan (`docs/backlog/2026-10-plan.md`, Oct 17) was to rebuild Encore part by part without changing how it looks and behaves: the same pages, words, settings and database, migrated in place and backed up before every upgrade. 0.26 (the daily-use fixes and the Rust web app at `/`) came first; the seven parts after it were built between Oct 17 and Oct 21 and ship in two releases, **0.30.0** (0.27 to 0.30, published) and **0.33.0** (0.31 to 0.33, next). Each part has its own notes in this folder; 0.30.0.md and 0.33.0.md sum them up.

| Part | Ships in | What it did | Database |
|---|---|---|---|
| 0.27 | 0.30.0 | A new desktop app (Iced, native, every song through Cadenza on this computer, with exclusive mode and the device's real rates); the JavaScript web UI deleted; players that can't decode everything get the original or lossless FLAC | 26 |
| 0.28 | 0.30.0 | The website works offline (Download for offline, Downloads, plays and likes sent later); updates and restart from Settings → Diagnostics | 26 |
| 0.29 | 0.30.0 | The server as one Cargo workspace of separate parts with dead code gone and less work per page (`docs/perf/0.29.md`); settings drawn from one list the server sends; storage problems wait instead of costing downloads; *Keep anyway*; Soulseek reconnects on its own schedule; torrents and Usenet fixed; the classic desktop app removed | 26 |
| 0.30 | 0.30.0 | The phone: every Settings page, one sound engine, songs saved in Music/Encore, updates itself, bug reports, playback through drops and restarts | 26 |
| 0.31 | 0.33.0 | Group listen; volume normalization; lyrics mode; *Liked from this artist* and *In your library*; one library search with initials | 27 |
| 0.32 | 0.33.0 | Plugins: sources, download clients, Cadenza, the Translator and the media-server connections installed and updated one by one; plugins by other people in a sandbox; the upgrade takes over what was in use | 28 |
| 0.33 | 0.33.0 | Files for admins; the car's four buttons and *Car buttons*; errors above every screen on the phone; shared playlists and adding other people's; *Download now*; download folders that can't be written; translated titles without the local model, DeepL | 29 |

### What is different for you
- **One web app, in Rust**, served at `/`; it works offline and asks to reload after an update. The server shows its own settings pages, so the website, the desktop app and the phone draw the same fields.
- **A native desktop app** for Windows, macOS and Linux that plays through Cadenza and updates itself; the classic app is gone.
- **The phone app** is the website's equal: every Settings page, offline songs, self-update, bug reports, group listen, Files, Plugins, *Download now*, shared playlists. Error messages show above everything with *Copy*.
- **The car** has four buttons you can rearrange, the quality under the title, *More by <artist>*, bigger lyrics in the cover and Encore Lyrics with the lyrics over the whole screen.
- **Downloads** are plugins you install and update without a new image. Problems with the disk make requests wait, with the reason and the folder, instead of failing or downloading twice. Each source's line is visible, and *Download now* moves a request up without breaking a source's limits.
- **Listening together and sharing**: group listen with guests from a link, and playlist links that always play the playlist as it is now.
- **Admins** get a file manager over the library and the download folders, with Trash and imports that keep files where they are.

### Removed on the way
The JavaScript web UI (`ENCORE_LEGACY_UI` does nothing), the classic Tauri desktop app, the Lidarr and HiFi settings, six API routes no app called, the image's own copies of yt-dlp and streamrip (now plugins), and the *Download tools* card (now Settings → Plugins).

### Before upgrading from 0.30.0
The database goes from 26 to 29 in one start, after an automatic backup; 0.30 can't open it afterwards, so going back means putting that backup back with the container stopped. The tools in use are taken over as plugins with no new setup. The details are under *Before you update* in 0.33.0.md.

### What only your hardware can confirm
- **Cadenza on Windows** (RTX 3070, Singxer SU-2, Fosi ZD3): exclusive mode, the DSD list stopping at DSD256, the link limit, the Opus song through the FLAC fallback, CUDA without dropouts (from 0.27).
- **The phone**: an hour of background playback through a server restart, an update installed from a release signed with the release key, offline songs on Android 8 or 9 (from 0.30).
- **The car**: the four buttons without ⋯, the *Car buttons* preview against the real row, Encore Lyrics' card and taps (0.33).
- **Your server**: the upgrade with your tools, the sandbox line on your Docker host, a read-only downloads mount, slskd's queue position for *Download now*, your Internet Archive plugin, a DeepL key (0.32 and 0.33).

### Known limits after 0.33
- A plugin you added doesn't update by itself to a release that asks for more access; its card names what the release asks for, and *Update* installs it without asking again.
- The car buttons' optional *Similar songs* and an *Artist* button that works offline were not built.
- The built-in romanization has no Chinese (no pinyin dictionary) and writes Korean without the sound changes between syllables; the local model or DeepL fill those in.
- The download addresses of the catalog's programs (slskd, streamrip, llama.cpp and the models) are checked by the *catalog-links* workflow, which has not had a real run on GitHub yet.
- The phone's own Requests link box still treats an Encore link as a request link; sharing it into the app from another app adds the playlist.
