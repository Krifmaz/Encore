# `web/js/player.js` — rewrite inventory

Source: `web/js/player.js` (672 lines). Imports: `api` from `./api.js`; `art, esc, fmt, icon, toast` from `./ui.js`.
This document replaces reading the file. Everything below (markup, text, keys, URLs, timings) must be reproduced byte-for-byte by the Leptos port.

---

## 1. Purpose and exports

The persistent bottom player bar (`#player`), the full-screen "now playing" view (`#np-full`, tabs Now playing / Lyrics / Up next), the play queue, two `<audio>` elements (current + pre-buffered next), library-shuffle continuation when the queue ends, synced lyrics, play logging/scrobble triggers, MediaSession, native-app bridge (`window.EncoreNative`), and the "linked remote" mode where this device only sends commands to another device (Encore 2 remote).

### Exports

| Export | Signature | Does | Callers |
|---|---|---|---|
| `shuffleLibrary` | `async () → number` | `freshShuffle(50)`; if non-empty `playQueue(first, 0, {context:'library'})`; returns count (0 ⇒ caller toasts "Your library is empty"). | `pages/library.js:141` (`#shuffle-lib`) |
| `onChange` | `(fn) → unsubscribe()` | Adds `fn` to `listeners`; `fn(current())` is called on every `emit()`. Returns `() => listeners.delete(fn)`. | `pages/library.js:433` (`ctx.onLeave(player.onChange(highlight))`) |
| `current` | `() → track \| null` | `state.queue[state.index] \|\| null`. | `remote.js:134`, `pages/library.js:434`, internal |
| `linked` | `() → link \| null` | Returns module `link`. | `remote.js` (many) |
| `setLink` | `(l \| null)` | Enter/leave remote-link mode (see §2 `link`, §9). | `remote.js:38,45,72` |
| `linkState` | `(s, online = true)` | New state from the linked device each heartbeat; re-renders bar only if `[id, playing, online]` changed, else just moves time. | `remote.js:62` |
| `snapshot` | `() → object` | What this device plays, for `/remote/heartbeat` (§6). | `remote.js:53,74,157` |
| `setVolume` | `(v)` | If linked: forward `{action:'volume', value:Number(v)}`, set `link.state.volume`. Else clamp 0..1, set `audio.volume` + `spare.volume`, persist `localStorage['encore.volume']`, set `#vol.value`. | internal (`runCommand 'volume'`, linked `#vol` onchange) |
| `command` | `(c)` | Sets `local = true`, `runCommand(c)`, `finally local = false` (commands received from another device are never forwarded back). | `remote.js:57,160` |
| `queue` | `() → {tracks: state.queue, index: state.index}` | Live references. | none found (unused export) |
| `playQueue` | `(tracks, start = 0, {context = 'list'} = {})` | Replace queue and play (or forward when linked). | `pages/library.js` (9, 230, 404, 405, 453), `pages/discover.js` (209, 293, 356), `pages/foryou.js` (33, 125), `pages/playlists.js:193`, `pages/home.js:89`, `pages/common.js:54`, `remote.js:150` |
| `enqueue` | `(tracks)` | Append to queue (or forward). | `pages/library.js:25,409` |
| `playNext` | `(tracks)` | Insert after current (or forward). | `pages/library.js:24,410` |
| `preview` | `(t)` | `playQueue([{...t, preview:true, id:'pv-' + t.preview_url}], 0, {context:'preview'})` — 30-s preview not in library. | none found (unused export; `library.js` has its own local `preview()`) |
| `canSaveOffline` | `() → boolean` | `!!window.EncoreNative?.saveOffline`. | `pages/library.js:39,412` |
| `saveOffline` | `(tracks)` | Android: per numeric-id track call `EncoreNative.saveOffline(JSON)` (§7). | `pages/library.js:39,412` |
| `seekTo` | `(sec)` | Seek (local or forwarded). | `remote.js:152`, internal |
| `toggle` | `()` | Play/pause (local or forwarded). | internal, `window.encorePlayer.toggle` |
| `next` | `async (auto = false)` | Advance (see §9 for full decision order). | internal, `window.encorePlayer.next`, MediaSession |
| `prev` | `()` | Restart or previous. | internal, `window.encorePlayer.prev`, MediaSession |
| `mount` | `()` | Create DOM, audio elements, global listeners; idempotent (`if #player exists return`). | `app.js:170` (in `boot()` after `renderShell()` and `remote.start(session)`, only when signed in) |
| `setFull` | `(on)` | Show/hide full-screen view; when linked and `on` → `window.encoreRemote?.open()` instead. | internal, `window.encorePlayer.open` |

### Internal functions (not exported)

`markPlayed(t)`, `freshShuffle(limit=50)`, `emit()`, `native()`, `viaLink(c)`, `slim(t)`, `runCommand(c)`, `cover(t)`, `sendable(tracks)`, `streamUrl(t, transcode=false, offset=0)`, `convert(t)`, `load(transcode=false, offset=0)`, `preloadNext()`, `topUp()`, `duration()`, `position()`, `continueWithLibrary()`, `jump(i)`, `removeAt(i)`, `moveTo(from,to)`, `shuffleUpNext()`, `logPlay()`, `updateTime()`, `quality(t)`, `bindSeek(seek, curId)`, `updateLinkedTime()`, `renderLinked(el)`, `render()`, `renderFull()`, `parseLrc(text)`, `lyricsHtml()`, `loadLyrics()`, `syncLyrics(force=false)`.

### Track object shape (as consumed)

| Field | Type | Used for |
|---|---|---|
| `id` | number (library) or string `pv-<url>` (preview) | identity; everything "library-only" tests `typeof id === 'number'` |
| `title`, `artist`, `album` | string (may be null → `esc` → `''`) | display, MediaSession, native |
| `album_id` | string | cover URL, album link `#/library/album/{album_id}` |
| `format` | string e.g. `flac`, `mp3`, `alac` | `convert()` decision, quality badge (uppercased) |
| `bitrate` | number (kbps) or falsy | quality badge `"<FMT> <bitrate>k"` |
| `duration` | number seconds | fallback duration, transcoded duration |
| `cover` | string URL | used only if no `album_id` or if preview |
| `preview` | `true` on preview tracks | 30-s preview behaviours |
| `preview_url` | string | stream URL for previews |

Server `/library/shuffle` rows have exactly: `id, title, artist, album, album_id, format, bitrate, duration`.

---

## 2. State

| Variable | Type / shape | Initial | Changes |
|---|---|---|---|
| `state.queue` | `track[]` | `[]` | `playQueue` (replace, `tracks.filter(Boolean)`), `enqueue` (push), `playNext` (splice after index), `topUp`/`continueWithLibrary` (push shuffle), `removeAt`, `moveTo`, `shuffleUpNext`, `#p-close` (→ `[]`) |
| `state.index` | number | `-1` | `playQueue` → `clamp(start, 0, len-1)`; `enqueue` when `-1` → `0`; `next`/`prev`/`jump`; `removeAt` (`--` if `i < index`); repeat-all → `0`; `#p-close` → `-1` |
| `state.transcoded` | boolean | `false` | `load()`: `!t.preview && transcode` (transcode forced true when `convert(t)`) |
| `state.offset` | number (s) | `0` | `load(_, offset)`; clock origin for converted streams restarted at a seek point |
| `state.dragging` | boolean | `false` | seek pointerdown/touchstart/input → `true`; change/pointerup/touchend → `false`; `load()` → `false` |
| `state.context` | `'list' \| 'album' \| 'single' \| 'library' \| 'preview'` | `'list'` | `playQueue` option; `continueWithLibrary` → `'library'`. Only `'preview'` changes behaviour (no top-up/continuation; `playQueue` with preview context never forwards to link) |
| `state.repeat` | `'off' \| 'all' \| 'one'` | `'off'` | `#f-repeat` cycles off→all→one→off. Not persisted |
| `state.full` | boolean | `false` | `setFull(on)` |
| `state.pane` | `'now' \| 'lyrics' \| 'queue'` | `'now'` | `#p-open` click → `'now'`; `#p-queue` → `'queue'`; `.np-tabs` click → `data-pane`. Not persisted |
| `state.logged` | boolean | `false` | `load()` with `offset == 0` → `false`; `logPlay` → `true` once threshold met |
| `state.announced` | track ref (added dynamically) | `undefined` | `load()` with `offset == 0`: if `typeof t.id === 'number' && state.announced !== t` → set and POST now-playing (identity compare on the object) |
| `listeners` | `Set<fn>` | empty | `onChange` |
| `audio` | `HTMLAudioElement` (`#audio`) | `null` | `mount()`; swapped with `spare` in `load()` when the pre-buffered URL matches |
| `spare` | `HTMLAudioElement` (`#audio-next`, `preload="auto"`) | `null` | `mount()`; swap; `preloadNext` sets `src` + `dataset.url` + `load()` |
| `lyr` | `{ id, lines: [{t:number, text:string}], plain: string, loading: boolean, instrumental?: boolean }` | `{id:null, lines:[], plain:'', loading:false}` | `loadLyrics()` |
| `played` | `Set<number>` | from `sessionStorage['encore.played']` (JSON array) or `[]` | `markPlayed` (add + persist); `freshShuffle` clears when server returns empty (`removeItem`) |
| `link` | `null \| { id, name, state: {}, online: boolean, send(cmd) }` | `null` | `setLink` |
| `local` | boolean | `false` | `command()` wraps `runCommand` |
| `linkTick` | interval id | `null` | `setLink`: `clearInterval` then, if `l`, `setInterval(1000)` advancing `link.state.position` by 1 while `link.state.playing` and calling `updateLinkedTime()` |
| `topping` | boolean | `false` | `topUp()` re-entrancy guard |
| `lastLine` | number | `-1` | `syncLyrics` (index of highlighted line); never reset on track change (force re-sync handles it) |
| `CONVERT` | `Set` | `['ape','wv','aiff','aif','wma','alac','dsf','dff','tta','mpc']` | constant; compared with `(t.format \|\| '').toLowerCase()` |

`link.state` (from the other device's heartbeat snapshot) fields read: `id, title, artist, album, cover, playing, position, duration, volume`.

---

## 3. Routes handled and navigation targets

No hash routes handled. `window 'hashchange'` → `if (state.full) setFull(false)` (any navigation closes the full-screen view).

Links emitted:

| Where | href |
|---|---|
| Empty bar text | `#/library` (`style="color:var(--accent)"`) |
| Full view artist (non-preview) | `#/artist-name/${encodeURIComponent(t.artist \|\| '')}` |
| Full view album (when `t.album_id`) | `#/library/album/${esc(t.album_id)}` |

---

## 4. DOM contract

### 4.1 `mount()` — elements appended to `<body>` (in this order)

1. `<audio id="audio" preload="auto">` — reused if an element with id `audio` already exists (appended only if not connected).
2. `<audio id="audio-next" preload="auto">`.
3. `<div class="player" id="player"></div>`.
4. `<div class="np-full hidden" id="np-full"></div>`.

Volume init: `audio.volume = spare.volume = parseFloat(localStorage['encore.volume'] ?? '0.9')` (0.9 default; errors ignored). Then `render()`.

### 4.2 Body classes

| Class | Set | Removed |
|---|---|---|
| `has-player` | `load()`, `render()` (always, even empty), `renderLinked()` | never |
| `np-open` | `setFull(true)` | `setFull(false)` |

`#np-full` toggles class `hidden` (`classList.toggle('hidden', !on)`).

### 4.3 Bar — empty state (`render()` with no current track and not linked)

```html
<div class="now"><div class="thumb"><div class="ph">${icon.music.replace('<svg', '<svg width="20"')}</div></div>
      <div class="grow"><div style="font-weight:650" class="faint">Nothing playing</div>
      <div class="small muted ellipsis">Open an album in <a href="#/library" style="color:var(--accent)">Library</a> and press ▶ Play</div></div></div>
      ${window.encoreRemote ? `<div class="right"><button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button></div>` : ''}
```
`#p-devices` click → `window.encoreRemote.open()`. If `state.full` → `setFull(false)`.

### 4.4 Bar — playing (`render()` with a current track `t`, not linked)

```html
<div class="now" id="p-open" role="button" tabindex="0" aria-label="Open the full-screen player" title="Open player">
  <div class="thumb">${art(cover(t), t.album || t.title)}</div>
  <div class="grow">
    <div class="ellipsis" style="font-weight:650">${esc(t.title)}</div>
    <div class="ellipsis small muted">${esc(t.artist)}${t.album ? ' · ' + esc(t.album) : ''}</div>
  </div>
</div>
<div class="ctrls">
  <div class="row">
    <button class="btn ghost icon" id="p-prev" aria-label="Previous">${icon.prev}</button>
    <button class="play-btn" id="p-toggle" aria-label="Play/Pause">${audio.paused ? icon.play : icon.pause}</button>
    <button class="btn ghost icon" id="p-next" aria-label="Next">${icon.next}</button>
  </div>
  <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
</div>
<div class="right">
  <span class="badge" title="${state.transcoded ? 'Converted to FLAC for your browser (lossless)' : t.preview ? 'A 30-second preview – not in your library' : 'Original file, no conversion'}">${esc(quality(t))}</span>
  <button class="btn ghost icon" id="p-queue" aria-label="Up next" title="Up next">${icon.list}</button>
  ${window.encoreRemote ? `<button class="btn ghost icon" id="p-devices" aria-label="Other devices" title="Play on or control your other devices">${icon.devices}</button>` : ''}
  <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
  <input type="range" id="vol" min="0" max="1" step="0.01" value="${audio.volume}">
  <button class="btn ghost icon" id="p-close" aria-label="Close player">${icon.x}</button>
</div>
```
(The literal is indented with 4 spaces/newlines inside `el.innerHTML`; whitespace text nodes are irrelevant to CSS but the element order is.) After render: `updateTime()`; `if (state.full) renderFull()`.

`quality(t)`: `''` if no track; `'30 s preview'` if preview; else `F = (t.format||'').toUpperCase()`; transcoded → `` `${F} → FLAC` ``; else `` `${F}${t.bitrate ? ' ' + t.bitrate + 'k' : ''}` `` (e.g. `MP3 320k`, `FLAC`).

### 4.5 Bar — linked (`renderLinked(el)`, when `link` is set; `s = link.state || {}`)

```html
<div class="now" id="p-open" role="button" tabindex="0" title="Your devices">
  <div class="thumb">${s.title ? art(s.cover, s.album || s.title) : `<div class="ph">${icon.devices.replace('<svg', '<svg width="20"')}</div>`}</div>
  <div class="grow">
    <div class="ellipsis" style="font-weight:650">${esc(s.title || 'Nothing playing')}</div>
    <div class="ellipsis small muted">${s.title ? esc(s.artist || '') + (s.album ? ' · ' + esc(s.album) : '') : 'Play anything – it starts there'}</div>
    <div class="ellipsis small">${where}</div>
  </div>
</div>
<div class="ctrls">
  <div class="row">
    <button class="btn ghost icon" id="p-prev" aria-label="Previous">${icon.prev}</button>
    <button class="play-btn" id="p-toggle" aria-label="Play/Pause">${s.playing ? icon.pause : icon.play}</button>
    <button class="btn ghost icon" id="p-next" aria-label="Next">${icon.next}</button>
  </div>
  <div class="seek"><span id="t-cur">0:00</span><input type="range" id="seek" min="0" max="1000" value="0"><span id="t-dur">0:00</span></div>
</div>
<div class="right">
  <button class="btn sm" id="p-unlink" title="Stop being a remote and play on this device again">${icon.link || ''}Unlink</button>
  <span class="faint vol-ic">${icon.volume.replace('<svg', '<svg width="18"')}</span>
  <input type="range" id="vol" min="0" max="1" step="0.05" value="${s.volume ?? 0.9}">
</div>
```
where `where` = `` `<span class="linked-to" title="This device is a remote for ${esc(link.name)}">${icon.link || ''}${link.online ? '' : 'Offline · '}Playing on ${esc(link.name)}</span>` ``.
Note: no quality badge, no `#p-queue`, no `#p-devices`, no `#p-close`; `#vol` step is `0.05` (vs `0.01` locally); `#p-open` has no `aria-label`. Ends with `updateLinkedTime()`.

### 4.6 Full-screen view (`renderFull()`; returns early if `#np-full` missing or no current track)

Sets inline custom property on `#np-full`: `--np-bg: url("${cover(t) || ''}")` (raw cover URL, no `?s=`).
`upNext = state.queue.slice(state.index + 1)`; `auto = localStorage['encore.autoplay'] !== 'off'`.

```html
<div class="np-backdrop"></div>
<div class="np-top">
  <button class="btn ghost icon" id="f-close" aria-label="Close">${icon.x}</button>
  <div class="tabs np-tabs"><button data-pane="now" class="${state.pane === 'now' ? 'active' : ''}">Now playing</button><button data-pane="lyrics" class="${state.pane === 'lyrics' ? 'active' : ''}">Lyrics</button><button data-pane="queue" class="${state.pane === 'queue' ? 'active' : ''}">Up next</button></div>
  <span></span>
</div>
<div class="np-body pane-${state.pane}">
  <section class="np-main">
    <div class="np-art">${art(cover(t), t.album || t.title, "big")}</div>
    <div class="np-meta">
      <h2 class="ellipsis">${esc(t.title)}</h2>
      <div class="muted ellipsis">${t.preview ? esc(t.artist) : `<a href="#/artist-name/${encodeURIComponent(t.artist || '')}">${esc(t.artist)}</a>`}${t.album ? ` · ${t.album_id ? `<a href="#/library/album/${esc(t.album_id)}">${esc(t.album)}</a>` : esc(t.album)}` : ''}</div>
      <span class="badge mt">${esc(quality(t))}</span>
    </div>
    <div class="seek np-seek"><span id="f-cur">0:00</span><input type="range" id="fseek" min="0" max="1000" value="0"><span id="f-dur">0:00</span></div>
    <div class="np-ctrls">
      <button class="btn ghost icon" id="f-shuffle" title="Shuffle up next" aria-label="Shuffle up next">${icon.shuffle}</button>
      <button class="btn ghost icon" id="f-prev" aria-label="Previous">${icon.prev}</button>
      <button class="play-btn lg" id="f-toggle" aria-label="Play/Pause">${audio.paused ? icon.play : icon.pause}</button>
      <button class="btn ghost icon" id="f-next" aria-label="Next">${icon.next}</button>
      <button class="btn ghost icon ${state.repeat !== 'off' ? 'on' : ''}" id="f-repeat" title="Repeat: ${state.repeat}" aria-label="Repeat">${icon.refresh}${state.repeat === 'one' ? '<sup>1</sup>' : ''}</button>
    </div>
    <div class="row np-vol"><span class="faint">${icon.volume.replace('<svg', '<svg width="18"')}</span><input type="range" id="fvol" min="0" max="1" step="0.01" value="${audio.volume}"></div>
  </section>
  <section class="np-lyrics" id="np-lyrics">${lyricsHtml()}</section>
  <section class="np-queue">
    <div class="row between"><h3>Up next</h3>
      <label class="row small muted" style="gap:6px" title="When the queue ends, keep playing a shuffle of the whole library"><input type="checkbox" id="f-auto" ${auto ? 'checked' : ''}> Then shuffle library</label></div>
    <div class="np-list">${ /* items or empty text, below */ }</div>
  </section>
</div>
```
Up-next items — `upNext.slice(0, 200)`, each with absolute queue index `i = state.index + 1 + k`:
```html
<div class="np-item" data-i="${i}">
  <div class="thumb">${art(cover(q), q.album || q.title)}</div>
  <div class="grow" style="min-width:0"><div class="ellipsis" style="font-weight:600">${esc(q.title)}</div><div class="small muted ellipsis">${esc(q.artist)}</div></div>
  <button class="btn ghost icon sm" data-up="${i}" aria-label="Move up" ${i === state.index + 1 ? 'disabled' : ''}>↑</button>
  <button class="btn ghost icon sm" data-rm="${i}" aria-label="Remove">${icon.x}</button></div>
```
Empty list: `<div class="muted small" style="padding:16px 4px">Nothing queued – a shuffle of your library plays next.</div>` when `auto`, else `…>Nothing queued.</div>`.
Title attribute of repeat button is literally `Repeat: off` / `Repeat: all` / `Repeat: one`. Ends with `updateTime()` and `syncLyrics(true)`.

### 4.7 Lyrics pane content (`lyricsHtml()`)

| Condition (first match wins) | HTML |
|---|---|
| no current track | `''` |
| `t.preview` | `<div class="muted np-empty">Lyrics are available for songs in your library.</div>` |
| `lyr.loading \|\| lyr.id !== t.id` | `<div class="muted np-empty"><span class="spinner"></span> Looking for lyrics…</div>` |
| `lyr.lines.length` | one `<p data-t="${l.t}" data-li="${i}">${esc(l.text) \|\| '♪'}</p>` per line (`t` is seconds as JS number, e.g. `12.34`, `60`) |
| `lyr.plain` | `<div class="np-plain">${esc(lyr.plain)}</div>` |
| else | `<div class="muted np-empty">Instrumental</div>` if `lyr.instrumental`, else `…>No lyrics found for this song.</div>` |

Active synced line gets class `on` (`p.on`), one at a time.

### 4.8 Helpers from `ui.js` whose output is part of the contract

* `esc(s)`: `String(s ?? '')` with `& < > " '` → `&amp; &lt; &gt; &quot; &#39;`.
* `art(src, label, cls='')`: `ph = <div class="ph">${esc(initials(label))}</div>`; if `!src` → `ph`; if `src` matches `/\/api\/(library\/albums\/[^/]+|share\/[^/]+)\/cover$/` append `?s=1000` when `cls === 'big'` else `?s=400`; returns `` `${ph}<img class="${cls}" src="${esc(src)}" alt="" loading="lazy" onerror="this.remove()">` `` (placeholder first, image on top).
* `initials(s)`: `(s||'?')` strip leading `/^the\s+/i`, trim, first char uppercased, or `'?'`.
* `fmt.dur(s)`: `s = Math.round(s||0)`; `h ? "h:MM:SS" : "M:SS"` (minutes not zero-padded unless hours present; seconds always 2 digits).
* `icon.*`: inline SVGs. Stroke icons are `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">…</svg>`: `music, devices, x, list, volume, shuffle, refresh, link`. Fill icons `<svg viewBox="0 0 24 24" fill="currentColor">…</svg>`: `play, pause, next, prev`. Paths (exact): see `web/js/ui.js` lines 6–46. The player uses `.replace('<svg', '<svg width="20"')` for `music`/`devices` placeholders and `'<svg width="18"'` for `volume`.
* `toast(msg, type)`: appends `<div class="toast ${type}">msg</div>` to `#toasts`; fades at 3800 ms, removed at 4200 ms.

### 4.9 CSS classes/ids relied upon (defined in `web/css/app.css`)

`body.has-player` (sets `--player`), `.player` (fixed bottom grid 1fr/1.4fr/1fr; ≤960px: 112px tall two-row grid, seek bar on row 1, hides `.right input/.faint/.badge` and `#p-prev`), `.player .now/.thumb/.grow/.ctrls/.row/.seek/.right`, `.player #seek` (custom range with `--p` gradient), `.play-btn`, `.play-btn.lg`, `.np-full`, `.np-backdrop` (uses `--np-bg`), `body.np-open`, `.np-top`, `.np-tabs`, `.np-body.pane-now|pane-lyrics|pane-queue` (controls which of `.np-main/.np-lyrics/.np-queue` show; ≤760px single pane), `.np-main`, `.np-art`, `.np-meta`, `.np-seek`, `#fseek` (`--p`), `.np-ctrls`, `.np-ctrls .btn.on`, `.np-ctrls sup`, `.np-vol`, `.np-lyrics p`, `.np-lyrics p.on`, `.np-plain`, `.np-empty`, `.np-queue h3`, `.np-item`, `.np-item .thumb`, `.linked-to`, `#p-unlink svg`, `.vol-ic`, plus generic `.btn .ghost .icon .sm .on`, `.tabs button.active`, `.badge`, `.mt`, `.hidden`, `.ellipsis .small .muted .faint .grow .row .between`, `.spinner`, `.thumb`, `.ph`.

---

## 5. Events and interactions

### 5.1 Global (installed once in `mount()`)

| Target | Event | Effect |
|---|---|---|
| `audio` & `spare` (filtered to `e.target === audio`) | `ended` | `logPlay(); next(true)` |
| 〃 | `play`, `pause` | `render(); native()` |
| 〃 | `timeupdate` | `updateTime(); logPlay(); syncLyrics()` |
| 〃 | `loadedmetadata` | `updateTime()` |
| 〃 | `error` | if `current() && !state.transcoded && !current().preview && audio.getAttribute('src')` → `load(true, position())` (retry as server FLAC) |
| `spare` | `error` | `delete spare.dataset.url` |
| `navigator.mediaSession` | `play`/`pause`/`nexttrack`/`previoustrack` | `audio.play()` / `audio.pause()` / `next()` / `prev` |
| 〃 (try/catch) | `seekto` → `seekTo(d.seekTime)`; `seekforward` → `seekTo(position() + (d.seekOffset \|\| 10))`; `seekbackward` → `seekTo(position() - (d.seekOffset \|\| 10))` |
| `document` | `keydown` | see 5.2 |
| `window` | `hashchange` | `if (state.full) setFull(false)` |

### 5.2 Keyboard (document `keydown`)

1. `Escape` while `state.full` → `setFull(false)`; return.
2. Return if `(!current() && !link)` or (`activeElement.tagName` matches `/INPUT|TEXTAREA|SELECT|BUTTON/` and its `id` does not match `/seek/`). So shortcuts work when focus is on body/links/`#seek`/`#fseek`, not on `#vol`, buttons, or form fields.
3. `Space` (`e.code`) → `preventDefault(); toggle()`.
4. `at = link ? (link.state?.position || 0) : position()`; `ArrowRight` → `preventDefault(); seekTo(at + 10)`; `ArrowLeft` → `seekTo(at - 10)`.

### 5.3 Bar (playing) listeners — set on each `render()`

| Element | Event | Effect |
|---|---|---|
| `#p-toggle` | click | `toggle` |
| `#p-next` | click | `next()` |
| `#p-prev` | click | `prev` |
| `#p-open` | click | `state.pane = 'now'; setFull(true)` |
| `#p-open` | keydown `Enter` | `setFull(true)` (pane unchanged) |
| `#p-queue` | click | `state.pane = 'queue'; setFull(true)` |
| `#p-devices` (optional) | click | `window.encoreRemote.open()` |
| `#seek` | `bindSeek(seek, 't-cur')` (5.5) |
| `#vol` | input | `audio.volume = spare.volume = value; localStorage['encore.volume'] = audio.volume` |
| `#p-close` | click | `audio.pause(); audio.removeAttribute('src'); spare.removeAttribute('src'); delete spare.dataset.url; state.queue = []; state.index = -1; render(); emit()` |

### 5.4 Bar (linked) listeners — set on each `renderLinked()`

| Element | Event | Effect |
|---|---|---|
| `#p-toggle` | click | `toggle` (forwards) |
| `#p-next` / `#p-prev` | click | `next()` / `prev` (forward) |
| `#p-open` | click | `window.encoreRemote?.open()` |
| `#p-unlink` | click | `window.encoreRemote?.unlink()` |
| `#vol` | change | `setVolume(e.target.value)` (forwards) |
| `#seek` | pointerdown | `state.dragging = true` |
| `#seek` | input | `state.dragging = true; #t-cur = fmt.dur(value/1000 * (s.duration \|\| 0)); --p = value/10 %` |
| `#seek` | change | `state.dragging = false; seekTo(value/1000 * (link.state?.duration \|\| 0))` |

### 5.5 `bindSeek(seek, curId)` (used for `#seek` locally and `#fseek`)

| Event | Effect |
|---|---|
| pointerdown, touchstart (`{passive:true}`) | `state.dragging = true` |
| input | `state.dragging = true; #curId.textContent = fmt.dur((value/1000) * duration()); seek.style.setProperty('--p', (value/10) + '%')` |
| change, pointerup, touchend | `commit`: if dragging → `dragging = false; seekTo((value/1000) * duration())` |

Range is `0..1000`; `--p` is set as a percentage string `value/10 + '%'`.

### 5.6 Full view listeners — set on each `renderFull()`

| Element | Event | Effect |
|---|---|---|
| `#f-close` | click | `setFull(false)` |
| `.np-tabs` | click (delegated `[data-pane]`) | `state.pane = pane; renderFull(); if pane === 'lyrics' loadLyrics()` |
| `#f-toggle` | click | `toggle` |
| `#f-next` / `#f-prev` | click | `next()` / `prev` |
| `#f-shuffle` | click | `shuffleUpNext` (Fisher-ish `sort(() => Math.random() - 0.5)` of `queue.slice(index+1)`, spliced back; `render(); emit(); preloadNext()`) |
| `#f-repeat` | click | `state.repeat = {off:'all', all:'one', one:'off'}[state.repeat]; renderFull()` |
| `#f-auto` | change | `localStorage['encore.autoplay'] = checked ? 'on' : 'off'; renderFull()` |
| `#fvol` | input | `audio.volume = spare.volume = value; if #vol exists → #vol.value = audio.volume` (**not** persisted to localStorage) |
| `#fseek` | `bindSeek(fseek, 'f-cur')` |
| `.np-list` | click (delegated) | `[data-rm]` → `removeAt(+rm)`; else `[data-up]` → `moveTo(+up, +up - 1)`; else `[data-i]` → `jump(+i)` (plays that queue entry) |
| `#np-lyrics` | click (delegated `[data-t]`) | `seekTo(+data-t)` |

Queue edits: `removeAt(i)` no-op if `i === state.index`; decrements index if `i < index`. `moveTo(from,to)` no-op if `from === index || to <= index || to >= queue.length` (so the first up-next item can't move; button also `disabled`). All three then `render(); emit(); preloadNext()` (which re-renders full view via `render()` when `state.full`).

### 5.7 Timers

| Timer | Where | Period / delay |
|---|---|---|
| `linkTick` | `setLink(l)` | every 1000 ms: if `link?.state?.playing` → `position += 1; updateLinkedTime()` |
| `setTimeout(preloadNext, 1500)` | end of `load()` | buffer next song after current starts |
| `loadedmetadata` once-listener | `runCommand 'play_queue'` with `position > 1` | `seekTo(position)` then remove |

### 5.8 Toasts

| Text | Kind | When |
|---|---|---|
| `Only songs from your library can be played on another device` | `err` | `playQueue` while linked and no track has numeric id |

(Toasts for Play next / Add to queue / Linked / Unlinked are shown by callers, not this file.)

### 5.9 Modals / menus

None created here. `setFull(true)` while linked, `#p-open` (linked), `#p-devices`, call `window.encoreRemote.open()` (the "Your devices" modal in `remote.js`).

---

## 6. API calls (via `api.js`: `fetch('/api' + path, {headers:{'X-Requested-With':'encore'}, credentials:'same-origin'})`, JSON body; non-2xx throws `ApiError(status, data.detail || statusText)`; dispatches `encore:server-down`/`encore:server-up`/`encore:unauthorized` window events)

| Method & path | Body | Response used | Errors |
|---|---|---|---|
| `POST /library/shuffle` | `{ limit: 50, exclude: number[] }` (`exclude` = union of `played` + numeric ids currently in queue) | array of track rows `{id,title,artist,album,album_id,format,bitrate,duration}`; if empty → clear `played` + `sessionStorage`, retry with `exclude: [currentId]` (or `[]`) | propagates to `shuffleLibrary` (caller handles) / swallowed in `topUp` & `continueWithLibrary` |
| `POST /library/now-playing` | `{ track_id }` | ignored | `.catch(() => {})` |
| `POST /library/plays` | `{ track_id, seconds: position() }` | ignored | `.catch(() => {})` |
| `GET /library/tracks/{id}/lyrics` | — | `{ lyrics: string\|null, synced: boolean, source, instrumental?: true }`; `synced` → `parseLrc(lyrics)`; else `plain = lyrics \|\| ''` | catch → `lyr = {id, lines:[], plain:'', loading:false}` (renders "No lyrics found for this song.") |

Media URLs (set as `src`, not fetched):

| URL | Built by |
|---|---|
| `/api/library/tracks/{id}/stream` | `streamUrl(t)` original file |
| `/api/library/tracks/{id}/stream?transcode=1` | transcoded (FLAC) |
| `/api/library/tracks/{id}/stream?transcode=1&t={offset.toFixed(1)}` | transcoded restarted at seek point (only when `offset` truthy) |
| `t.preview_url` | preview tracks (external) |
| `/api/library/albums/{album_id}/cover` | `cover(t)` (then `art()` appends `?s=400`/`?s=1000`; MediaSession artwork and `--np-bg` use it raw) |
| `/api/library/albums/{album_id}/cover?s=600` | `saveOffline` art (absolute via `new URL(..., location.href)`) |
| `/api/library/tracks/{id}/stream?transcode=true` | `saveOffline` url when `convert(t)` (absolute) |

Server note: stream responds `audio/flac` + header `x-transcoded: flac` when converting; `t` query is seconds.

---

## 7. Browser APIs

| API | Key / use | Format |
|---|---|---|
| `sessionStorage` | `encore.played` | JSON array of numeric track ids heard this tab session; read once at module load; written on every `markPlayed`; removed when a shuffle round completes |
| `localStorage` | `encore.volume` | `String(audio.volume)` e.g. `"0.9"`, `"0.35"`; read in `mount()` with `parseFloat(... ?? '0.9')`; written by `#vol` input and `setVolume` (not by `#fvol`) |
| `localStorage` | `encore.autoplay` | `'on'` / `'off'`; absent ⇒ on. Read in `topUp`, `continueWithLibrary`, `renderFull`; written by `#f-auto` |
| `HTMLAudioElement` ×2 | `src`, `removeAttribute('src')`, `getAttribute('src')`, `play()` (promise rejection swallowed), `pause()`, `load()`, `currentTime`, `duration`, `paused`, `volume`, `preload='auto'`, `dataset.url` (URL currently loaded, for swap detection), `isConnected` |
| `navigator.mediaSession` | `metadata = new MediaMetadata({title, artist, album, artwork: cover ? [{src: cover(t), sizes:'512x512'}] : []})` on each `load()`; action handlers listed in 5.1 | strings default `''` |
| `window.EncoreNative.nowPlaying(json)` | called from `native()` on every `emit()`, `play`, `pause` (only if function exists and `audio` exists) | `{"title","artist","album","playing":bool,"art": absolute cover URL or ""}` or `{"stopped":true}` |
| `window.EncoreNative.saveOffline(json)` | `saveOffline` per track | `{"id": String(id), "title","artist","album", "ext": conv ? "flac" : (format \|\| "audio"), "url": absolute stream URL, "art": absolute cover?s=600 or ""}` |
| `window.encorePlayer` (assigned at module load) | `{ toggle, next, prev, open: () => setFull(true) }` — for native shells |
| `window.encoreRemote` (read; provided by `remote.js` when feature `remote` enabled) | `open()`, `unlink()` |
| `location.href` | base for `new URL()` absolute URLs |
| `document.body.classList`, `document.activeElement`, `Element.scrollTo({top, behavior})` | lyrics centering: `top = cur.offsetTop - box.clientHeight/2 + cur.offsetHeight/2`, `behavior: force ? 'auto' : 'smooth'` |

Not used here: Notification, Clipboard, File, service worker (registered in `app.js`).

---

## 8. Cross-file dependencies and global side effects

| File | Symbols used by player.js |
|---|---|
| `web/js/api.js` | `api.post`, `api.get` |
| `web/js/ui.js` | `art`, `esc`, `fmt.dur`, `icon.{music, devices, prev, play, pause, next, list, volume, x, shuffle, refresh, link}`, `toast` |

| Consumer | Imports from player.js |
|---|---|
| `web/js/app.js` | `mount` |
| `web/js/remote.js` | `setLink`, `linked`, `snapshot`, `command`, `linkState`, `current`, `playQueue`, `seekTo` |
| `web/js/pages/library.js` | `playQueue`, `playNext`, `enqueue`, `canSaveOffline`, `saveOffline`, `shuffleLibrary`, `onChange`, `current` |
| `web/js/pages/discover.js`, `foryou.js`, `playlists.js`, `home.js` (dynamic import), `pages/common.js` (dynamic import) | `playQueue` |

Global side effects: `window.encorePlayer = {...}` at import time; appends `#audio`, `#audio-next`, `#player`, `#np-full` to `<body>`; `document` keydown and `window` hashchange listeners; `body.has-player` / `body.np-open`; MediaSession handlers; `sessionStorage`/`localStorage` writes; calls `window.EncoreNative.*` and `window.encoreRemote.*`. No custom events dispatched by this file (api.js dispatches `encore:server-down/up/unauthorized`).

Remote protocol objects produced here (sent via `link.send` → `POST /remote/{id}/command` in remote.js): `{action:'toggle'}`, `{action:'next'}`, `{action:'prev'}`, `{action:'seek', value}`, `{action:'volume', value}`, `{action:'enqueue', tracks: slim[]}`, `{action:'play_next', tracks: slim[]}`, `{action:'play_queue', tracks: slim[], index}`. Accepted by `runCommand`: those plus `play`, `pause`, `stop` (= pause), `play_queue` with optional `index`, `position`. `slim(t)` = `{id, title, artist, album, album_id, format, duration}`. `snapshot()` = `{...slim(cur), cover: cover(cur), playing, position, duration, volume, index, queue: queue.filter(numeric id).slice(0,300).map(slim)}` or `{playing:false}` when nothing/no audio.

---

## 9. Gotchas and exact behaviours

**Playback engine**
* `load(transcode=false, offset=0)`: `markPlayed(t)`; `transcode ||= convert(t)`; `state.transcoded = !t.preview && transcode`; `state.offset = offset`; `dragging = false`; if `offset == 0`: `logged = false` and now-playing POST (identity-guarded by `state.announced`). URL = `streamUrl(t, transcoded, offset)`. **Swap**: if `!offset && spare.dataset.url === url` → `audio ↔ spare`; old one `pause()`, `removeAttribute('src')`, `delete dataset.url`; new `audio.volume = old.volume; currentTime = 0`. Else `audio.src = url`. Then `audio.dataset.url = url; audio.play().catch(()=>{})`; MediaSession metadata; `body.has-player`; `render(); emit()`; `if (state.full) loadLyrics()`; `topUp()`; `setTimeout(preloadNext, 1500)`.
* Listeners are attached to both audio elements once; handlers run only when `e.target === audio` (the variable, re-read at event time) — after a swap the old element's events are ignored.
* `preloadNext()`: next = `queue[index+1]`; `url = streamUrl(n, convert(n))` (no offset); skip if `spare.dataset.url === url`; set `dataset.url`, `src`, `load()`.
* `duration()`: preview → `audio.duration` if finite else `30`; transcoded → `t.duration || 0` (server stream has no duration); else `audio.duration` if finite else `t.duration || 0`. `position()` = `state.offset + (audio.currentTime || 0)`.
* `seekTo(sec)`: linked → forward, set `link.state.position = sec`, `updateLinkedTime()`. Local: needs `current()` and `dur`; clamp `[0, dur - 0.25]`; transcoded → `load(true, sec)` (server restarts conversion at `t=sec.toFixed(1)`); else `audio.currentTime = sec`; `updateTime()`.
* `toggle()`: linked → forward + optimistic `link.state.playing = !playing; render()`. Local: if `current()`, `paused ? play() : pause()`.
* `next(auto)`: (1) if `!auto` and linked → forward, stop. (2) `auto && repeat === 'one'` → `seekTo(0); audio.play()`. (3) `index < len-1` → `index++; load()`. (4) `repeat === 'all' && len` → `index = 0; load()`. (5) `context === 'preview'` → stop. (6) `await continueWithLibrary()` → `index++; load()`.
* `prev()`: linked → forward. `position() > 3 || index === 0` → `seekTo(0)`; else `index--; load()`.
* `error` on `audio`: fallback to server FLAC at the current position (`load(true, position())`) — once, since `state.transcoded` then true.
* `#p-close` leaves `body.has-player` set and re-renders the empty bar (so the layout keeps the bar).
* `audio.volume` default 0.9; `#vol` writes `localStorage` on every `input`; `#fvol` does not persist (only mirrors to `#vol`).

**Queue continuation**
* `topUp()`: skip if `topping`, `context === 'preview'`, or `index < len - 2` (runs when on the last or second-to-last song); skip if autoplay off; appends `freshShuffle(50)`; `render(); emit(); preloadNext()`; errors swallowed.
* `continueWithLibrary()` (from `next()` at queue end): autoplay check; `freshShuffle(50)`; push; `context = 'library'`; returns true/false (false on error/empty).
* `freshShuffle(limit)`: exclude = `played ∪ queued numeric ids`; if result empty → clear `played` (+ sessionStorage), re-request excluding only the current id → new round.
* `markPlayed` only for numeric ids and only once per id.

**Play logging (`logPlay`, on `timeupdate` and `ended`)**: skip if no track, preview, already logged, or non-numeric id. With `d = duration()`, `p = position()`: if `d` → log when `d < 30 ? p >= d - 1 : p >= min(d/2, 240)`; if `!d` → `p >= 30`. Then `logged = true` and `POST /library/plays {track_id, seconds: p}`.

**Linked mode**
* `setLink(l)`: if newly linking and local audio playing → `audio.pause()`; reset 1-s ticker; close full view if open; `render()`. `link.state` is replaced wholesale by `linkState(s)`; `online` false shows `Offline · ` prefix in `.linked-to`.
* While linked: `playQueue` (except `context 'preview'` or when `local`) converts to `slim` list filtered to numeric ids, `index = indexOf(first.id)`, forwards `play_queue`; `enqueue`/`playNext` forward only if any numeric-id track (`out.length && viaLink(...)`). `setVolume`, `seekTo`, `toggle`, `next` (non-auto), `prev` forward. Commands received via `command()` run locally even while linked (`local = true` disables forwarding).
* `updateLinkedTime()` skipped while dragging; `pos = min(position, duration || ∞)`.
* Keyboard arrows use `link.state.position` as base when linked.

**Full view**
* `setFull(true)` while linked → opens devices modal instead and leaves `state.full` false. `setFull(true)` without a current track shows an empty `#np-full` (renderFull returns early) — `render()`'s empty state then calls `setFull(false)` on the next render.
* `renderFull()` re-creates the whole subtree on every state change (`render()` calls it when `state.full`), including on every `play/pause` event, queue edit, repeat/auto toggle, tab switch.
* `loadLyrics()`: if no track / preview / `lyr.id === t.id` → just repaint `#np-lyrics`. Else set `lyr = {id, lines:[], plain:'', loading:true}`, paint spinner, `GET lyrics`; discard response if track changed meanwhile; paint + `syncLyrics(true)`. Called on `setFull(true)`, on `load()` while full, and when switching to the Lyrics tab.
* `parseLrc(text)`: split on `/\r?\n/`; timestamps `/\[(\d+):(\d+(?:\.\d+)?)\]/g` → `t = min*60 + sec`; `text` = line with all `[...]` tags stripped and trimmed; one entry per timestamp (multi-stamp lines duplicate text); lines without timestamps (e.g. `[ar:…]` metadata) produce nothing; sorted by `t` ascending (stable).
* `syncLyrics(force)`: only when `state.full`, synced lines exist, and `lyr.id === current().id`; `p = position() + 0.3` (300 ms look-ahead); `i` = last line with `t <= p` (or -1); skip if `i === lastLine && !force`; remove `.on` from all `p.on`, add to `p[data-li="i"]`, scroll container so the line is vertically centered (`auto` when forced, `smooth` otherwise).
* Up-next list shows at most 200 entries; `data-i` are absolute queue indices; clicking a row plays it (`jump`).

**Formatting / text**
* Times via `fmt.dur` (`M:SS`, `H:MM:SS`), rounded to whole seconds. Range inputs `0..1000` with `--p` as `value/10 + '%'`.
* Quality badge: `30 s preview` | `<FMT> → FLAC` | `<FMT>[ <bitrate>k]`. Badge `title` has three fixed strings (4.4).
* Exact punctuation to preserve: ` · ` (middle dot separators), `–` (en dash in "preview – not in your library", "Nothing queued – a shuffle…", "Play anything – it starts there", "Offline · "), `…` in "Looking for lyrics…", `▶` in the empty-bar hint, `↑` on move-up buttons, `♪` for empty synced lines, `→` in `FLAC` conversion badge, `<sup>1</sup>` on repeat-one.
* All user data passes through `esc()`; `album_id` in href is `esc`'d (not URL-encoded); artist in href is `encodeURIComponent`'d.
* No i18n/plurals in this file.

**Ordering**
* `app.js boot()`: `renderShell()` → `remote.start(session)` (may call `setLink` **before** `mount()`, so `render()` is a no-op until `#player` exists; `mount()` then renders the linked bar) → `player.mount()`.
* `window.encorePlayer` is set at import time, before `mount()`; its methods guard on `current()`/`audio` indirectly (`toggle` checks `current()`; `next/prev/open` tolerate empty state except `prev` which calls `position()` → requires `audio`; only reachable after mount in practice).
