# Encore vs Roon

What Roon is known for, what Encore already does in that area, and where each is ahead. Written for the
"replace Roon with Encore" question; the gaps at the end are the roadmap.

| Area | Roon | Encore today | Who's ahead |
|---|---|---|---|
| **Library browsing** | Albums, artists, composers, works, genres; editorial metadata (bios, reviews, credits, liner notes) licensed from Rovi/TiVo and others | Albums, artists, songs, genres, playlists; MusicBrainz tagging of everything, artist bios and similar artists from Last.fm, release lists merged from MusicBrainz + Deezer + Apple + Discogs | Roon for editorial depth (credits, reviews). Encore for *discography completeness* (what exists vs what you own) |
| **Getting music** | None – Roon plays what you have (local + Tidal/Qobuz/KKBOX) | The whole point: request anything → Qobuz/Tidal/Deezer, Soulseek (built in), torrents/Usenet, YouTube Music; whole albums, upgrades to lossless, fingerprint verification, duplicates, tagging, artwork | Encore (Roon has nothing here) |
| **Streaming services** | Tidal, Qobuz, KKBOX fully integrated in the library | Imports playlists from Spotify, Apple Music, Deezer, Tidal, Qobuz, YouTube Music, SoundCloud, Bandcamp, ListenBrainz; downloads them; two-way like/playlist sync with Jellyfin, Plex, Navidrome, ListenBrainz, Spotify, YouTube Music | Roon for *streaming inside the app*; Encore for *owning* the music and syncing it everywhere |
| **Recommendations** | Valence (collaborative filtering), Roon Radio, Daily Mixes | For you (Last.fm + your plays), Made for you (ListenBrainz Daily/Weekly Jams, Weekly Exploration), similar artists, library shuffle that never repeats, Auto-download of new exploration picks | Even. Encore's are from open services and can download what they suggest |
| **Multi-room / endpoints** | RAAT to RoonReady devices, AirPlay, Chromecast, Squeezebox, HQPlayer; sub-ms sync; grouped zones | Every signed-in device (browser, phone, desktop app, car) is a player and a remote; instant push over the server's event stream; send/link between devices; Roon zones controllable from Encore; **Cadenza** devices (coming) for exclusive-mode output | Roon for synchronised groups and certified hardware. Encore for "any screen is a player" |
| **DSP** | MUSE: upsampling to DSD512, parametric EQ, convolution, crossfeed, headroom, per-zone | In the browser, free for the server: parametric EQ, crossfeed, balance, mono, crossfade (all optional; off = bit-exact to the OS). On the server (admin opt-in, per-user permission): Cadenza upsampling with a render cache. Cadenza on your own box: long-FIR upsampling (millions of taps), dither / noise shaping, convolution, parametric EQ, bit-perfect switch, exclusive-mode output; controlled from Encore's Playback settings | Roon today; Cadenza is built to pass it (HQPlayer-class filters, which MUSE lacks) |
| **Signal path** | Yes, with quality rating | Yes: file → what Encore sent → what the device did; linked devices report theirs; Cadenza reports every stage | Even |
| **Playback quality** | Bit-perfect to RAAT endpoints | Bit-perfect to the browser/OS (original file streamed); lossless conversion only for formats a browser can't play; Auto quality only when the connection can't keep up | Even for the file; Roon wins at the DAC until Cadenza |
| **Mobile** | Roon ARC (remote streaming, downloads, CarPlay/Android Auto) | PWA + Android app + desktop app; offline saving; Android Auto (browse, voice, car player); share links with previews | Even (Roon has CarPlay; Encore has no iOS app) |
| **Lyrics** | Yes (synced for some) | Yes: synced LRC from LRCLIB, saved next to the file | Even |
| **Scrobbling** | Last.fm | Last.fm and ListenBrainz, from every player including the car | Encore |
| **Multi-user** | One profile set per Core, no per-user permissions | Users, roles, invites, request approvals, per-user connections, play stats, leaderboard | Encore |
| **Self-hosting** | Core on your machine, but account, licensing and metadata depend on Roon's cloud; subscription | One container, SQLite, no account with anyone, no subscription | Encore |
| **Interface** | Polished native apps (Windows, macOS, Linux, iOS, Android) | Web UI (JavaScript and a Rust/WebAssembly build), Tauri desktop, Android app | Roon for polish; Encore for running anywhere |

## What Roon does that Encore doesn't (yet)

1. **Synchronised multi-room groups** – Encore devices play independently. Cadenza could add a sync protocol between Cadenza endpoints later.
2. **RoonReady / certified streamer support** – Encore can *control* Roon's zones through the extension, but cannot stream to a RoonReady box without Roon. Cadenza on a Raspberry Pi next to the DAC is the open answer.
3. **Editorial metadata** (credits, reviews, "who played on this") – licensed data; Encore could add MusicBrainz relationships (performer credits are there) and Wikipedia/Discogs notes.
4. **Streaming catalogue inside the library** – Encore prefers to download; Tidal/Qobuz browsing without downloading isn't a goal.
5. **iOS app** – the PWA works; there is no native iOS app.

## What Encore does that Roon doesn't

Acquiring music (requests, sources, verification, tagging, upgrades), Soulseek built in, multi-user with approvals, two-way sync with media servers and services, ListenBrainz/Last.fm from every player, playlist import from everywhere, shared song links, no subscription, and – with Cadenza – HQPlayer-class filtering as free software.
