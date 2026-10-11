# Encore web shell – inventory for the Rust/WebAssembly rewrite

Scope: the app shell of the plain-ES-module web app in `web/` (served by the Rust axum server from
`ENCORE_STATIC_DIR`; the Leptos build is served beside it under `/next/`). This document replaces reading these
files. Everything a user can see or do here must be reproduced 1:1 (DOM, classes, text, API calls, storage keys,
hash routes). `web/css/app.css` is reused verbatim, so the DOM must match its selectors.

| File | Lines | Role |
|---|---|---|
| `web/index.html` | 21 | static skeleton: `#app`, `#toasts`, `#modal-root`, loads `/js/app.js` |
| `web/js/app.js` | 206 | boot, session, hash router, sidebar shell, badges, connection banner, global events |
| `web/js/api.js` | 39 | `fetch` wrapper for `/api/*`, `ApiError`, server-up/down/unauthorized events |
| `web/js/appearance.js` | 110 | theme / accent / background / width → `<html data-*>` + CSS variables, logo SVG |
| `web/js/ui.js` | 272 | `esc`, icons, toast, modal, confirm, formatters, art, badges, popMenu, password eye buttons |
| `web/sw.js` | 25 | service worker: network-first shell cache |
| `web/share.html` | 49 | standalone public "shared song" page (served at `/s/{token}`) |
| `web/manifest.webmanifest` | 24 | PWA manifest |
| `web/css/app.css` | 558 | the stylesheet (selector inventory in §9) |

Total: 1304 lines.

Conventions used below: "quoted" HTML is the template literal verbatim (JS `${…}` kept where the value is
dynamic); `esc()` = `ui.esc` HTML-escaping; `ctx` = the object passed to page renderers.

---

## 0. Cross-cutting contract

### 0.1 Body-level DOM (ids that must exist / be appended exactly like this)

| Node | Who creates it | Notes |
|---|---|---|
| `div#app` | index.html | shell (signed in) **or** auth page (signed out) rendered inside with `innerHTML` |
| `div#toasts` | index.html | `ui.toast` appends `div.toast[.ok\|.err]`; CSS fixes it bottom-right above the player |
| `div#modal-root` | index.html | `ui.modal` appends `div.modal-bg`; `app.route()` empties it on every navigation |
| `audio#audio`, `audio#audio-next` | player.js `mount()` | appended to body |
| `div.player#player`, `div.np-full.hidden#np-full` | player.js `mount()` | appended to body after the shell exists |
| `div#conn-banner` | app.js `connBanner` | appended to body while offline / server down |
| `div.menu-bg` + `div.menu-pop[role=menu]` | ui.popMenu | appended to body |
| `span.pw-wrap` / `button.eye-btn` | ui.js MutationObserver | wraps every `input[type=password]` and `[data-secret]` anywhere in body |

### 0.2 Boot order (app.js `boot()`)
1. `appearance.apply()` (from `localStorage['encore.appearance']`, before any paint).
2. `GET /api/auth/info` → `session.info` (on error `{}`).
3. `GET /api/auth/me` → `session.user` (on error `null`; a 401 here does **not** fire `encore:unauthorized` because the path starts with `/auth/`).
4. If signed in: `appearance.adoptAccount(user)` (account prefs win), `renderShell()`, `remote.start(session)`, `player.mount()`, `refreshPendingBadge()` + `setInterval(refreshPendingBadge, 30000)`.
5. `route()`.

### 0.3 Custom `window` events

| Event | Dispatched by | Listened by | Effect |
|---|---|---|---|
| `encore:server-down` | api.js (network error, or HTTP 502–504) | app.js | `connBanner(true, "The Encore server isn't answering")` |
| `encore:server-up` | api.js (any other response) | app.js | `connBanner(false)` |
| `encore:unauthorized` | api.js (401 on a non-`/auth/` path) | app.js | if signed in: `session.user=null`, toast `Your session expired – please sign in again` (err), `location.hash='#/login'`, `location.reload()` |
| `encore:requests-changed` | pages (common.js, picker.js, download.js, foryou.js, discover.js, home.js) | app.js | `refreshPendingBadge()` |
| `encore-native` (`CustomEvent`, `detail:{text, kind?}`) | desktop/Android wrapper | app.js | `toast(detail.text, detail.kind \|\| 'ok')` if `detail.text` |
| `hashchange` | browser; app.js `redirect()` fires it manually | app.js `route`; player.js (closes full-screen) | navigation |
| `offline` / `online` | browser | app.js | `connBanner(true)` with default text / `connBanner(false)` |

### 0.4 `window.*` globals

| Global | Set by | Read by |
|---|---|---|
| `window.encoreSession` (= exported `session`) | app.js | library.js (`encoreSession?.user?.perms?.fix`) |
| `window.encorePlayer {toggle,next,prev,open}` | player.js | native wrappers |
| `window.encoreRemote {open, unlink}` | remote.js `start()` (only if `info.features` includes `'remote'`) | player.js |
| `window.EncoreNative` (provided by the Tauri desktop/Android host) | host | app.js `openOffline`; auth.js `changeServer`; player.js `saveOffline`, `nowPlaying`; remote.js `saveOffline`, `platform` |

### 0.5 Web storage keys (all of them; the shell owns the first three)

| Key | Store | Format | Owner |
|---|---|---|---|
| `encore.appearance` | localStorage | JSON `{theme, accent, custom, background, width}` (see §6) | appearance.js |
| `encore.theme` | localStorage | legacy `'dark'\|'light'`; migrated once into `encore.appearance` | appearance.js |
| `encore.volume` | localStorage | stringified float 0–1 (default `'0.9'`) | player.js |
| `encore.autoplay` | localStorage | `'on'\|'off'` (anything but `'off'` = on) | player.js |
| `encore.played` | sessionStorage | JSON array of track ids heard this tab | player.js |
| `encore.device` | localStorage | random device id string | remote.js |
| `encore.device.name` | localStorage | user-given device name (≤60 chars) | remote.js |
| `encore.link` | localStorage | JSON `{id, name}` of the device this one remotely controls | remote.js |

### 0.6 Hash routes (app.js `routes`, matched against the path part of `location.hash.slice(1)`; `''` → `/`)

| Regex | Handler (file.export) | `params` |
|---|---|---|
| `^/$` | home.render | – |
| `^/search$` | discover.renderSearch | – (`query.q` used by the page) |
| `^/for-you$` | foryou.render | – |
| `^/tag/(.+)$` | foryou.renderTag | `[tag]` |
| `^/artist/([\w-]+)$` | discover.renderArtist | `[mbid]` |
| `^/artist-name/(.+)$` | discover.renderArtistByName | `[name]` (URL-encoded by callers) |
| `^/album/([\w-]+)$` | discover.renderAlbum | `[mbid]` |
| `^/library$` | library.render | – |
| `^/library/album/([\w-]+)$` | library.renderAlbum | `[albumId]` |
| `^/requests$` | requests.render | – |
| `^/playlists$` | playlists.render | – |
| `^/playlists/(\d+)$` | playlists.renderPlaylist | `[id]` |
| `^/connections$` | `redirect('connections')` | → `#/settings?s=connections` |
| `^/following$` | `navigate('/for-you')` | |
| `^/download$` | download.render | – |
| `^/activity$` | `navigate('/download')` | |
| `^/admin/users$` | `redirect('users')` | → `#/settings?s=users` |
| `^/admin/invites$` | `redirect('invites')` | |
| `^/settings$` | settings.render | `query.s` selects the sub-page |
| `^/admin/autodownload$` | `redirect('autodownload')` | |
| `^/admin/diagnostics$` | `redirect('diagnostics')` | |
| `^/account$` | `redirect('account')` | |
| `^/storage$` | storage.render | – |
| `^/leaderboard$` | leaderboard.render | – |
| no match | inline "Page not found" | |

Signed-out paths are handled by `pages/auth.js render(root, {path, query, session})`: `/reset` (needs `query.token`),
`/magic` (needs `query.token`), `/signup` or any path with `query.invite` (except `/login`) → sign-up form,
everything else (incl. `/login`, `/`) → sign-in form. Other hash paths are not special when signed out.

---

## 1. `web/index.html`

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <meta name="theme-color" content="#0b0b12">
  <title>Encore</title>
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="manifest" href="/manifest.webmanifest">
  <link rel="apple-touch-icon" href="/icons/apple-touch-icon.png">
  <meta name="apple-mobile-web-app-capable" content="yes">
  <meta name="mobile-web-app-capable" content="yes">
  <link rel="stylesheet" href="/css/app.css">
</head>
<body>
  <div id="app"></div>
  <div id="toasts"></div>
  <div id="modal-root"></div>
  <script type="module" src="/js/app.js"></script>
</body>
</html>
```

- `meta[name=theme-color]` is rewritten by `appearance.apply()` to the computed `--bg`.
- `document.title` is `Encore` until `renderShell()` sets it to `session.info.app_name || 'Encore'` (auth pages never change it).
- `web-rs/index.html` is identical except the module script is replaced by `<link data-trunk rel="rust" data-wasm-opt="z" />`; Trunk serves it with `public_url = "/next/"`.
- Server: `ServeDir(static_dir).fallback(index.html)`; `/next/*` → `ServeDir(static_dir/next).fallback(next/index.html)`; `/api/*` JSON; `/s/{token}` → `share.html`.

## 2. `web/manifest.webmanifest`

```json
{ "name": "Encore", "short_name": "Encore",
  "description": "Your music server: request, download and play your library.",
  "id": "/", "start_url": "/", "scope": "/", "display": "standalone", "orientation": "any",
  "background_color": "#0b0b12", "theme_color": "#0b0b12", "categories": ["music", "entertainment"],
  "icons": [ {"src": "/icons/icon-192.png", "sizes": "192x192", "type": "image/png"},
             {"src": "/icons/icon-512.png", "sizes": "512x512", "type": "image/png"},
             {"src": "/icons/maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable"},
             {"src": "/favicon.svg", "sizes": "any", "type": "image/svg+xml"} ],
  "shortcuts": [ {"name": "Library", "url": "/#/library", "icons": [{"src": "/icons/icon-192.png", "sizes": "192x192"}]},
                 {"name": "For you", "url": "/#/for-you", "icons": [{"src": "/icons/icon-192.png", "sizes": "192x192"}]},
                 {"name": "Downloader", "url": "/#/download", "icons": [{"src": "/icons/icon-192.png", "sizes": "192x192"}]} ] }
```
The app name in the manifest is the fixed string `Encore` (not the server's `app_name`). Shortcuts are hash routes.

## 3. `web/sw.js` (service worker)

| Item | Value |
|---|---|
| Cache name | `encore-shell-v4` (bump to invalidate) |
| Pre-cached `SHELL` | `/`, `/index.html`, `/css/app.css`, `/js/app.js`, `/favicon.svg`, `/manifest.webmanifest`, `/icons/icon-192.png` |
| `install` | `caches.open(CACHE).addAll(SHELL)` then `self.skipWaiting()` |
| `activate` | delete every cache whose key ≠ `CACHE`, then `self.clients.claim()` |
| `fetch` | **ignored** (passes through) when method ≠ GET, origin ≠ `location.origin`, or `pathname.startsWith('/api/')`. Otherwise network-first: `fetch(request)`; if `res.ok && (res.type === 'basic' \|\| 'default')` put a clone in the cache; return `res`. On network failure → `caches.match(request)` → else `caches.match(url.pathname.startsWith('/next/') ? '/next/index.html' : '/index.html')`. |
| Registration (app.js) | only when `'serviceWorker' in navigator && (location.protocol === 'https:' \|\| location.hostname === 'localhost')`: `navigator.serviceWorker.register('/sw.js').catch(() => {})` |

Gotchas: scope is `/` so it also controls `/next/…`; streams (`/api/...`) are never cached; the Leptos build must
register the same `/sw.js` (or none) – registering a second worker under `/next/` would fight this one. The
`/next/index.html` fallback is only useful if that file was fetched once while online (nothing pre-caches it).

## 4. `web/share.html` (standalone public page, not part of the SPA)

Served by the server at `/s/{token}` (`text/html; charset=utf-8`). Own inline CSS (dark only, not `app.css`):
`:root{color-scheme:dark; --bg:#0b0b12; --panel:#161622; --text:#f3f3f8; --muted:#9a9ab0; --accent:#8b5cf6}`,
`body` centred grid with `radial-gradient(circle at 30% 0%, #2a1b4a, var(--bg) 60%)`, `font: 15px/1.5 system-ui…`,
`.card{width:min(420px,100%); background:var(--panel); border-radius:20px; padding:22px; box-shadow:0 30px 80px rgba(0,0,0,.5); text-align:center}`,
`.art{position:relative; aspect-ratio:1; border-radius:14px; overflow:hidden; background:#22223a}` + `.art img` absolute cover,
`h1{font-size:22px; margin:18px 0 2px}`, `.muted`, `audio{width:100%; margin-top:16px}`, `.foot{margin-top:16px; font-size:13px}`, `a{color:var(--accent)}`, `.err{padding:30px 0}`.
Head: `<meta name="robots" content="noindex">`, `<title>Shared song</title>`, `<link rel="icon" href="/favicon.svg">`.

Initial DOM: `<div class="card" id="card"><div class="muted">Loading…</div></div>`.

Script (IIFE): `token = location.pathname.split('/').pop()`; own `esc` (same table as ui.esc).
- `GET /api/share/${encodeURIComponent(token)}` → `{title, artist, album?, app_name?}`; on `!r.ok` throws `(await r.json()).detail || 'Not found'`.
- Success: `document.title = `${t.title} – ${t.artist}``; `card.innerHTML =`
  ```html
  <div class="art"><img src="/api/share/${token}/cover" alt="" onerror="this.remove()"></div>
  <h1>${esc(t.title)}</h1><div class="muted">${esc(t.artist)}${t.album ? ' · ' + esc(t.album) : ''}</div>
  <audio controls preload="metadata" src="/api/share/${token}/stream"></audio>
  <div class="foot muted">Shared from ${esc(t.app_name || 'Encore')} · <a href="/#/login">Sign in</a> for the whole library</div>
  ```
  (`${token}` is `encodeURIComponent(token)`.) `audio.onerror` → once: `a.src += '?transcode=true'` (if `src` does not already contain `transcode`).
- Failure: `<div class="err"><h1>Link not available</h1><div class="muted">${esc(e.message)}</div></div>`.

This page can stay as static HTML in the rewrite (no module imports).

---

## 5. `web/js/api.js`

### 5.1 Exports
| Export | Signature | Behaviour | Callers |
|---|---|---|---|
| `ApiError` | `class extends Error { status: number; data: any\|null }` | thrown by every failed request | pages read `e.status`, `e.data` (e.g. `ex.data?.two_factor` in auth.js) |
| `api.get` | `(path) => Promise<any>` | `request('GET', path)` – no body | everywhere |
| `api.post` | `(path, body = {})` | `request('POST', path, body)` – always sends JSON (default `{}`) | |
| `api.put` | `(path, body = {})` | PUT, always JSON | |
| `api.patch` | `(path, body = {})` | PATCH, always JSON | |
| `api.del` | `(path)` | DELETE, no body | |

### 5.2 `request(method, path, body)` – exact algorithm
1. `opts = { method, headers: { 'X-Requested-With': 'encore' }, credentials: 'same-origin' }`. If `body !== undefined`: `headers['Content-Type'] = 'application/json'`, `opts.body = JSON.stringify(body)`.
2. `res = await fetch('/api' + path, opts)`. `path` is used verbatim (callers pass a leading `/` and any query string). On a thrown fetch (network down): dispatch `encore:server-down`, `throw new ApiError(0, 'Not connected to the server')`.
3. Dispatch `encore:server-down` if `502 ≤ status ≤ 504`, otherwise `encore:server-up` (on **every** response).
4. `text = await res.text()`; `data = text ? JSON.parse(text) : null`; if parsing throws, `data = text` (raw string).
5. If `!res.ok`: if `status === 401 && !path.startsWith('/auth/')` dispatch `encore:unauthorized`. `msg = (data && data.detail) || res.statusText || 'Request failed'`; if `Array.isArray(msg)` → `msg.map(m => m.msg).join(', ')` (legacy FastAPI validation errors). `throw new ApiError(status, msg, data)`.
6. Return `data`.

### 5.3 Gotchas
- The server's CSRF check requires `X-Requested-With: encore` on every cookie-authenticated mutating request.
- Session is a cookie (`credentials: 'same-origin'`); nothing is stored client-side.
- `GET` and `DELETE` never set `Content-Type`; `POST/PUT/PATCH` always do, even for `{}`.
- Empty body (204 / empty 200) → `null`.
- Error messages shown to users come from `data.detail` (server `err(status, msg)`).

---

## 6. `web/js/appearance.js`

### 6.1 Exports
| Export | Type / signature | Purpose | Callers |
|---|---|---|---|
| `ACCENTS` | `[{id, name, a, b}]` ×9 | preset accent pairs (gradient start/end) | settings.js (swatches, preset previews), `apply` |
| `PRESETS` | `[{id, name, theme, accent, background, custom?}]` ×5 | one-tap looks | settings.js |
| `DEFAULTS` | `{ theme:'dark', accent:'violet', custom:'#8b5cf6', background:'fade', width:'comfortable' }` | | settings.js, `current/save` |
| `logo(size = 30)` | `→ string` SVG | brand logo in accent colours | app.js `renderShell`, auth.js (brand) |
| `current()` | `→ prefs` | `{...DEFAULTS, ...JSON.parse(localStorage['encore.appearance'] \|\| '{}')}`; any exception → `{...DEFAULTS}` | settings.js, `apply` default arg |
| `apply(p = current())` | `→ void` | writes `<html data-theme/data-bg/data-width>` + CSS variables + theme-color meta | app.js `boot`, settings.js live preview, `save`, system-theme listener |
| `save(p, { serverPrefs = false } = {})` | `async` | `clean = {...DEFAULTS, ...p}`; `localStorage.setItem(KEY, JSON.stringify(clean))` (try/catch); `apply(clean)`; if `serverPrefs`: `api.put('/auth/prefs', { appearance: clean })`, errors swallowed | settings.js (`serverPrefs` = server has `features.includes('theme')`), `adoptAccount` |
| `adoptAccount(user)` | | `a = user?.prefs?.appearance`; if it is an object → `save(a)` (local only) | app.js `boot` |

Data:
```js
ACCENTS = [
  { id: 'violet', name: 'Violet', a: '#8b5cf6', b: '#ec4899' },
  { id: 'encore', name: 'Encore', a: '#f2a7e4', b: '#9fd2fb' },
  { id: 'lilac',  name: 'Lilac',  a: '#b48cf2', b: '#7cc4fa' },
  { id: 'pink',   name: 'Pink',   a: '#ec4899', b: '#f97316' },
  { id: 'sky',    name: 'Sky',    a: '#0ea5e9', b: '#8b5cf6' },
  { id: 'teal',   name: 'Teal',   a: '#14b8a6', b: '#0ea5e9' },
  { id: 'green',  name: 'Green',  a: '#22c55e', b: '#14b8a6' },
  { id: 'amber',  name: 'Amber',  a: '#f59e0b', b: '#ef4444' },
  { id: 'red',    name: 'Red',    a: '#ef4444', b: '#ec4899' },
];
PRESETS = [
  { id: 'default', name: 'Encore dark', theme: 'dark',  accent: 'violet', background: 'fade' },
  { id: 'oled',    name: 'OLED black',  theme: 'oled',  accent: 'violet', background: 'solid' },
  { id: 'pastel',  name: 'Pastel',      theme: 'dark',  accent: 'encore', background: 'fade' },
  { id: 'light',   name: 'Daylight',    theme: 'light', accent: 'sky',    background: 'fade' },
  { id: 'mono',    name: 'Mono',        theme: 'oled',  accent: 'custom', custom: '#e5e5e5', background: 'solid' },
];
```

### 6.2 State
| Variable | Type | Initial | Changes |
|---|---|---|---|
| `logoN` | int | 0 | `++` per `logo()` call → gradient ids `lg1`, `lg2`, … (unique per SVG instance on the page) |
| `mq` | `MediaQueryList\|undefined` | undefined | set once when `apply` runs with `theme === 'system'`; `change` → `apply(current())` |
| `KEY` | const | `'encore.appearance'` | |

Prefs shape (`p`): `theme ∈ 'dark'|'oled'|'light'|'system'`, `accent ∈ ACCENTS.id | 'custom'`, `custom` hex string,
`background ∈ 'fade'|'solid'`, `width ∈ 'comfortable'|'wide'`. Unknown values fall back as in `apply`.

### 6.3 `apply(p)` – exact rules
- `dark = p.theme === 'system' ? matchMedia('(prefers-color-scheme: dark)').matches : p.theme !== 'light'`
- `html.dataset.theme = !dark ? 'light' : p.theme === 'oled' ? 'oled' : 'dark'` (never `'system'`).
- `preset = ACCENTS.find(x => x.id === p.accent)`; `a = preset ? preset.a : (p.custom || DEFAULTS.custom)`; `b = preset ? preset.b : partner(a)`.
- CSS vars on `html.style`: `--accent: a`, `--accent-2: b`, `--accent-rgb: "r, g, b"` (decimal, comma+space), `--accent-2-rgb`, `--on-accent: (luminance(a)+luminance(b))/2 > 0.45 ? '#16161f' : '#fff'`.
- `html.dataset.bg = p.background === 'solid' ? 'solid' : 'fade'`; `html.dataset.width = p.width === 'wide' ? 'wide' : 'comfortable'`.
- `meta[name=theme-color].content = getComputedStyle(html).getPropertyValue('--bg').trim() || '#0b0b12'`.

Helpers: `rgb(hex)` (accepts `#abc` and `#aabbcc`) → `[r,g,b]`; `toHex(r,g,b)` clamps/rounds, 2-digit lowercase hex;
`luminance(hex)` = WCAG relative luminance (sRGB linearisation, weights .2126/.7152/.0722);
`partner(hex)`: RGB→HSL, `h = (h + 0.1) % 1` (hue +36°), same s and l, HSL→RGB → hex (gradient partner for custom colours).

### 6.4 Module side effects
On import: `old = localStorage['encore.theme']`; if set and `KEY` absent → `localStorage[KEY] = JSON.stringify({...DEFAULTS, theme: old})` (try/catch).

### 6.5 `logo(size)` output (exact)
```html
<svg class="logo" viewBox="16 15 580 580" width="${size}" height="${size}" aria-hidden="true"><defs><linearGradient id="${id}" x1="71" y1="351" x2="551" y2="257" gradientUnits="userSpaceOnUse"><stop offset="0" style="stop-color:var(--accent)"/><stop offset="1" style="stop-color:var(--accent-2)"/></linearGradient></defs><path d="P1" fill="url(#${id})"/><path d="P2" fill="url(#${id})"/><path d="P3" fill="url(#${id})"/></svg>
```
- P1 = `M249.205 114.329 L510.699 32.377 C510.699 32.377 525.463 25.913 538.636 31.22 C551.81 36.528 551.358 49.709 551.358 49.709 L550.464 99.763 C550.464 99.763 550.049 113.161 538.636 121.587 C527.224 130.012 514.95 133.407 514.95 133.407 L308.537 197.811 C298.175 201.165 275.923 206.119 273.697 229.869 C273.697 229.869 275.227 403.645 275.227 443.582 C275.227 493.942 241.622 582.872 154.358 582.872 C65.76 582.872 47.709 513.389 47.709 470.406 C47.709 412.557 102.003 373.129 140.138 369.574 C178.273 366.019 204.127 385.087 204.127 385.087 L201.373 166.271 C199.204 125.931 244.035 116.052 249.205 114.329 Z`
- P2 = `M345.928 420.623 C345.928 420.623 518.078 368.274 535.733 363.243 C553.388 358.212 564.789 368.142 564.274 383.837 L564.274 419.323 C564.274 449.986 539.212 459.963 525.178 464.079 L348.687 519.256 C329.357 525.979 320.186 517.423 320.186 498.97 L320.186 446.803 C320.186 430.002 326.687 425.907 345.928 420.623 Z`
- P3 = `M339.143 254.402 L494.664 206.043 C507.578 202.394 520.795 206.043 520.795 221.613 L520.795 239.527 C520.795 274.769 508.753 302.967 450.793 319.539 L341.142 356.25 C330.333 359.74 319.414 354.699 320.186 340.976 L319.414 282.394 C319.414 269.641 322.93 259.403 339.143 254.402 Z`
(Same three paths as `web/favicon.svg`, which draws them in `#f0ead6` on a `#1c1c1e` rounded square.)

### 6.6 API
`PUT /api/auth/prefs` body `{ appearance: {theme, accent, custom, background, width} }` (only from `save(…, {serverPrefs:true})`); response ignored. Account prefs arrive inside `GET /api/auth/me` as `user.prefs.appearance`.

### 6.7 Gotchas
- `apply` must run before first paint (theme flash). In Leptos: run in `main()` before mounting.
- `adoptAccount` → `save` writes localStorage and re-applies, but never pushes back to the server.
- `theme-color` is read from the computed `--bg`, which depends on the `data-theme` just set (synchronous, fine).
- Gradient ids must be unique per `<svg>` on the page (sidebar brand + auth card can co-exist only one at a time, but settings previews may render many logos).

---

## 7. `web/js/ui.js`

### 7.1 Exports
| Export | Signature | Does | Used by (files) |
|---|---|---|---|
| `esc` | `(s) => string` | `String(s ?? '')` with `& < > " '` → `&amp; &lt; &gt; &quot; &#39;` | all |
| `qs` | `(sel, el = document) => Element\|null` | `el.querySelector` | app.js |
| `qsa` | `(sel, el = document) => Element[]` | spread of `querySelectorAll` | (none of the listed files) |
| `icon` | `{name: svgString}` | 40 inline icons (§7.2) | all |
| `toast` | `(msg, type = '') => void` | §7.3 | all |
| `modal` | `({title, body, footer = '', wide = false, onMount}) => {el, close}` | §7.4 | admin, artwork, connections, discover, foryou, library, lyrics, picker, playlists, remote |
| `confirmDialog` | `(title, text, okLabel = 'Confirm', danger = false) => Promise<boolean>` | §7.5 | admin, common, connections, discover, library, playlists |
| `fmt` | `{dur, longDur, size, ago, date, num}` | §7.6 | most pages, player, remote, storage |
| `initials` | `(s) => string` | first letter (skipping leading "The ") upper-cased, or `?` | `art` |
| `art` | `(src, label, cls = '') => string` | §7.7 | common, discover, download, foryou, home, leaderboard, library, picker, playlists, player, remote |
| `STATUS` | `{status: [badgeClass, label]}` | §7.8 | `statusBadge` |
| `statusBadge` | `(status) => string` | §7.8 | common, discover, foryou, playlists |
| `availability` | `(item) => string` | §7.8 | discover |
| `debounce` | `(fn, ms = 300) => fn` | trailing debounce via `setTimeout` | discover, library |
| `emptyState` | `(ic, title, text = '') => string` | `<div class="empty">${ic}<h3>${esc(title)}</h3>${text ? `<p class="muted">${text}</p>` : ''}</div>` (text is raw HTML) | many pages |
| `skeletonGrid` | `(n = 12) => string` | `<div class="grid">` + n× `<div><div class="skeleton" style="aspect-ratio:1"></div><div class="skeleton" style="height:14px;margin-top:10px;width:70%"></div></div>` + `</div>` | discover, library |
| `copyText` | `async (t)` | `navigator.clipboard.writeText(t)` → toast `Copied to clipboard` (ok); failure → toast `Copy failed – select and copy manually` (err) | admin, diagnostics, library, settings |
| `popMenu` | `(anchor, items) => close` | §7.9 | common, library |
| `EYE`, `EYE_OFF` | svg strings | §7.10 | settings.js |
| `setSecretText` | `(el, value) => void` | §7.10 | settings.js, internal observer |

### 7.2 Icons
Wrapper `P(d)` = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`. The four transport icons are filled instead (`fill="currentColor"`, no stroke attributes).

| name | inner markup (inside `P(...)` unless noted) |
|---|---|
| home | `<path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>` |
| search | `<circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/>` |
| library | `<path d="M4 4v16M9 4v16M14 4l6 16"/>` |
| inbox | `<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>` |
| download | `<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5M12 15V3"/>` |
| activity | `<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>` |
| users | `<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/>` |
| ticket | `<path d="M2 9a3 3 0 0 0 0 6v2a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-2a3 3 0 0 0 0-6V7a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2z"/><path d="M13 5v2M13 17v2M13 11v2"/>` |
| settings | `<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>` |
| play (filled) | `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12-7.5a1 1 0 0 0 0-1.72l-12-7.5A1 1 0 0 0 7 4.5z"/></svg>` |
| pause (filled) | `<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>` |
| next (filled) | `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M5 5.5v13a1 1 0 0 0 1.5.86L16 13.5V18a1 1 0 0 0 2 0V6a1 1 0 0 0-2 0v4.5L6.5 4.64A1 1 0 0 0 5 5.5z"/></svg>` |
| prev (filled) | `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M19 5.5v13a1 1 0 0 1-1.5.86L8 13.5V18a1 1 0 0 1-2 0V6a1 1 0 0 1 2 0v4.5l9.5-5.86A1 1 0 0 1 19 5.5z"/></svg>` |
| plus | `<path d="M12 5v14M5 12h14"/>` |
| check | `<path d="M20 6 9 17l-5-5"/>` |
| x | `<path d="M18 6 6 18M6 6l12 12"/>` |
| menu | `<path d="M3 6h18M3 12h18M3 18h18"/>` |
| logout | `<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>` |
| tag | `<path d="M20.59 13.41 13.42 20.58a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><circle cx="7" cy="7" r="1.5"/>` |
| edit | `<path d="M12 20h9M16.5 3.5a2.12 2.12 0 1 1 3 3L7 19l-4 1 1-4z"/>` |
| trash | `<path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>` |
| refresh | `<path d="M21 12a9 9 0 1 1-2.64-6.36L21 8M21 3v5h-5"/>` |
| link | `<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>` |
| music | `<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>` |
| disc | `<circle cx="12" cy="12" r="10"/><circle cx="12" cy="12" r="3"/>` |
| user | `<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>` |
| sun | `<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>` |
| moon | `<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>` |
| copy | `<rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>` |
| volume | `<path d="M11 5 6 9H2v6h4l5 4zM15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/>` |
| external | `<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6M15 3h6v6M10 14 21 3"/>` |
| shield | `<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>` |
| heart | `<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/>` |
| spark | `<path d="M12 3l1.9 5.6L19.5 10l-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.4z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>` |
| chevron | `<polyline points="6 9 12 15 18 9"/>` |
| more | `<circle cx="12" cy="5" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="12" cy="19" r="1.6"/>` |
| shuffle | `<path d="M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/>` |
| list | `<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>` |
| devices | `<rect x="2" y="4" width="13" height="10" rx="2"/><path d="M6 18h5M8.5 14v4"/><rect x="17" y="8" width="5" height="12" rx="1.5"/><path d="M19.5 17h.01"/>` |

Icons carry no `width/height`; CSS sizes them by context (`.btn svg 16px`, `.nav a svg 18px`, `.empty svg 46px`, `#conn-banner svg 16px`, …). `availability()` injects `width="12" height="12"` into `icon.check`.

### 7.3 `toast(msg, type = '')`
`div.toast` with className `toast ${type}` (so `"toast "` with trailing space when type is empty), `textContent = msg` (never HTML), appended to `#toasts`. After 3800 ms: `style.opacity='0'; style.transition='opacity .3s'`; after 4200 ms: removed. Kinds in use: `''`, `'ok'`, `'err'`.

### 7.4 `modal({title, body, footer = '', wide = false, onMount})`
Appends to `#modal-root`:
```html
<div class="modal-bg">
  <div class="modal ${wide ? 'wide' : ''}" role="dialog" aria-modal="true">
    <div class="modal-head"><h2>${title}</h2><button class="btn ghost icon" data-close aria-label="Close">${icon.x}</button></div>
    <div class="modal-body">${body}</div>
    ${footer ? `<div class="modal-foot">${footer}</div>` : ''}
  </div>
</div>
```
`title`, `body`, `footer` are raw HTML (callers escape). Behaviour: `close()` removes `.modal-bg` and the document `keydown` listener; `Escape` → close; `mousedown` on the backdrop itself (`e.target === bg`) → close; every `[data-close]` inside (head button and any in the footer) → close on click. `onMount({el, close})` runs after insertion. Returns `{ el: .modal element, close }`. Multiple modals can stack (each appends).

### 7.5 `confirmDialog(title, text, okLabel = 'Confirm', danger = false)`
`modal({ title: esc(title), body: `<p class="muted" style="margin:0">${esc(text)}</p>`, footer: `<button class="btn" data-close>Cancel</button><button class="btn ${danger ? 'danger' : 'primary'}" data-ok>${esc(okLabel)}</button>` })`.
`[data-ok]` click → `done = true; close(); resolve(true)`. A `MutationObserver` on `#modal-root` (childList) resolves `false` once `m.el` leaves `document.body` and `done` is false (covers Cancel, ×, Escape, backdrop and `route()` clearing `#modal-root`).

### 7.6 `fmt`
| fn | Rule | Examples |
|---|---|---|
| `dur(s)` | `s = Math.round(s \|\| 0)`; `h = ⌊s/3600⌋, m = ⌊(s%3600)/60⌋, sec = s%60`; `h ? `${h}:${mm}:${ss}` : `${m}:${ss}`` (m unpadded, sec/min padded to 2) | 65 → `1:05`; 3600 → `1:00:00` |
| `longDur(s)` | `h = ⌊s/3600⌋, m = round((s%3600)/60)`; `h ? `${h} hr ${m} min` : `${m} min`` (m is rounded, so it can read `60 min` / `1 hr 60 min`) | 5400 → `1 hr 30 min`; 3599 → `60 min` |
| `size(b)` | `!b` → `'0 B'`; units `B KB MB GB TB`; `i = min(⌊log1024 b⌋, 4)`; `(b / 1024**i).toFixed(i > 1 ? 1 : 0) + ' ' + unit` | 1536 → `2 KB`; 1.5 MiB → `1.5 MB` |
| `ago(ts)` (seconds) | `!ts` → `never`; `d = now/1000 − ts`; `<60` → `just now`; `<3600` → `${⌊d/60⌋}m ago`; `<86400` → `${⌊d/3600⌋}h ago`; `<30 d` → `${⌊d/86400⌋}d ago`; else `new Date(ts*1000).toLocaleDateString()` | |
| `date(ts)` | `ts ? new Date(ts*1000).toLocaleString() : '—'` | |
| `num(n)` | `(n \|\| 0).toLocaleString()` | |
Locale-dependent output (`toLocaleString`) – in Rust use `Intl`/`js_sys::Date::to_locale_string` to match.

### 7.7 `art(src, label, cls = '')`
`ph = `<div class="ph">${esc(initials(label))}</div>``. `!src` → `ph`. If `src` matches `/\/api\/(library\/albums\/[^/]+|share\/[^/]+)\/cover$/` → append `?s=${cls === 'big' ? 1000 : 400}`. Return `ph + `<img class="${cls}" src="${esc(src)}" alt="" loading="lazy" onerror="this.remove()">`` – placeholder stays underneath; the image covers it (CSS `.thumb > img`, `.tile .art > img` … absolute). `initials`: `(s || '?').replace(/^the\s+/i, '').trim().slice(0, 1).toUpperCase() || '?'`.

### 7.8 Status badges
```js
STATUS = { pending: ['warn', 'Awaiting approval'], queued: ['info', 'Queued'], searching: ['info', 'Searching'],
  monitoring: ['accent', 'Monitoring'], downloading: ['accent', 'Downloading'], importing: ['accent', 'Importing'],
  completed: ['ok', 'Available'], failed: ['err', 'Failed'], denied: ['err', 'Denied'], cancelled: ['', 'Cancelled'] };
```
`statusBadge(status)`: `[cls, label] = STATUS[status] || ['', status]`; spinner for `searching|downloading|importing|monitoring`: `<span class="spinner" style="width:10px;height:10px;border-width:2px"></span>`; returns `<span class="badge ${cls}">${spin}${esc(label)}</span>`.
`availability(item)`: `item.in_library` → `<span class="badge ok">${icon.check with width="12" height="12"} In library</span>`; else `item.request_status` → `statusBadge(...)`; else `''`.

### 7.9 `popMenu(anchor, items)`
`items: [{ label, icon?, onClick?, danger?, href?, download? } | '-' | falsy]`. Removes any existing `.menu-pop, .menu-bg`. Creates `div.menu-bg` and `div.menu-pop[role=menu]`; `items.filter(Boolean)` rendered in order (index `i` into the filtered list):
- `'-'` → `<hr>`
- with `href` → `<a role="menuitem" class="${danger ? 'danger' : ''}" href="${esc(href)}" ${download ? 'download' : ''} data-i="${i}">${icon || ''}<span>${esc(label)}</span></a>`
- else → `<button role="menuitem" class="${danger ? 'danger' : ''}" data-i="${i}">${icon || ''}<span>${esc(label)}</span></button>`
Events: `Escape` → close; click on `.menu-bg` → close; click inside → `closest('[data-i]')`, close, then if `onClick`: `preventDefault()` + `onClick()` (links without `onClick` navigate/download natively). Both appended to `document.body`. Desktop positioning (when **not** `matchMedia('(max-width: 760px)')`): `r = anchor.getBoundingClientRect()`, `left = max(8, min(r.right − w, innerWidth − w − 8))`, `top = (r.bottom + h + 8 > innerHeight) ? max(8, r.top − h − 6) : r.bottom + 6` (px). On phones CSS turns it into a bottom sheet. Returns `close`.

### 7.10 Password / secret eye buttons
```js
EYE = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>'
EYE_OFF = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.9 17.9A10.4 10.4 0 0 1 12 19c-6.4 0-10-7-10-7a18.5 18.5 0 0 1 5.1-5.9M9.9 5.2A9.6 9.6 0 0 1 12 5c6.4 0 10 7 10 7a18.6 18.6 0 0 1-2.2 3.2M14.1 14.1a3 3 0 1 1-4.2-4.2"/><path d="m2 2 20 20"/></svg>'
```
- `eyeButton(onToggle)`: `<button type="button" class="eye-btn" aria-label="Show" title="Show">EYE</button>`; click → `preventDefault`, `stopPropagation`, toggle `shown`, innerHTML `EYE_OFF`/`EYE`, `title`/`aria-label` = `Hide`/`Show`, `onToggle(shown)`.
- `wrapPassword(input)`: `input.dataset.eye = '1'`; `span.pw-wrap` (+ class `grow` if the input has `.grow`; copies `input.style.maxWidth`) inserted in place of the input, input moved inside, eye button appended; toggle sets `input.type = shown ? 'text' : 'password'`. Result: `<span class="pw-wrap [grow]"><input type="password" data-eye="1" …><button class="eye-btn" …></button></span>`.
- `setSecretText(el, value)`: `el.dataset.value = value`; `el.textContent = el.dataset.shown === '1' ? value : '•'.repeat(clamp(value.length, 8, 32))`; if the next sibling is not already `.eye-btn`: `el.dataset.eye='1'` and `el.after(eyeButton(...))` whose toggle sets `el.dataset.shown = '1' | ''` and the text accordingly.
- `enhanceSecrets(root)`: `root.querySelectorAll('input[type=password]:not([data-eye])')` → `wrapPassword`; `'[data-secret]:not([data-eye])'` → `setSecretText(el, el.dataset.value ?? el.textContent)`.
- Module init: `enhanceSecrets(document.body)` then a `MutationObserver(childList, subtree)` on `document.body`; for each added element node: `enhanceSecrets(n.matches('input[type=password]') ? n.parentNode : n)`. Deferred to `DOMContentLoaded` if `document.body` is not there yet.

Gotcha for Leptos: this observer mutates DOM that a framework may own. Either render `.pw-wrap` + `.eye-btn` explicitly in every password field component (recommended) or keep the observer but never re-render a wrapped input's parent reactively.

---

## 8. `web/js/app.js`

### 8.1 Exports
| Export | Signature | Does | Callers |
|---|---|---|---|
| `session` | `{ user: object\|null, info: object\|null }` | the signed-in user (`GET /auth/me`) and server info (`GET /auth/info`); also `window.encoreSession` | pages via `ctx.user` / `window.encoreSession`; account.js imports it dynamically |
| `navigate(path)` | `location.hash = '#' + path` | hash navigation | passed to pages as `ctx.navigate`; common.js imports it |
| `avatarHtml(u, size = 32)` | `→ string` | §8.4 | renderShell; account.js (`avatarHtml(me, 52)`), admin.js (users table) via `await import('../app.js')` |
| `boot()` | `async` | §0.2 | called once at module end |
| `refreshUser()` | `async` | `session.user = await api.get('/auth/me'); renderShell(); route();` | account.js after profile/avatar changes |

Non-exported: `routes`, `cleanup`, `redirect`, `parseHash`, `navLink`, `renderShell`, `refreshPendingBadge`, `route`, `connBanner`.

### 8.2 State
| Variable | Type | Initial | Changes |
|---|---|---|---|
| `session.info` | `{app_name, signup_mode, version, build, server:'rust', features:[…], email_login}` or `{}` | null | set once in `boot` |
| `session.user` | public user: `{id, username, role:'user'\|'admin'\|'owner', email, avatar_at:number\|null, totp_enabled, perms:{auto_approve_playlists,pick,fix,storage,auto_approve,…}, is_admin, prefs:{appearance?…}, requests_this_week, disabled, created_at, …}` | null | `boot`, `refreshUser`, set to `null` on `encore:unauthorized` |
| `cleanup` | `Array<() => void>` | `[]` | page `onLeave(fn)` pushes; `route()` runs + clears |
| interval: pending badge | 30 000 ms | started in `boot` when signed in | never cleared |
| interval: storage meter | 300 000 ms | started in **every** `renderShell()` call | never cleared (duplicates after `refreshUser`) |

`features` currently sent by the server: `owner, totp, email_login, logout_all, avatar, theme, remote, rename, reveal`.

### 8.3 Routing internals
- `parseHash()`: `raw = location.hash.slice(1) || '/'`; `[path, q = ''] = raw.split('?')` (a second `?` is dropped); `query = Object.fromEntries(new URLSearchParams(q))`.
- `redirect(page)`: `history.replaceState(null, '', `#/settings?s=${page}`)` then `window.dispatchEvent(new HashChangeEvent('hashchange'))` (replaceState does not fire hashchange; the current history entry is replaced).
- `route()` (async):
  1. run every `cleanup` fn in try/catch, `cleanup = []`; `#modal-root.innerHTML = ''`.
  2. `{path, query} = parseHash()`.
  3. if `!session.user` → `authPages.render(qs('#app'), { path, query, session })`; return.
  4. if `!qs('#view')` → `renderShell()` (e.g. the auth page was in `#app`).
  5. `#sidebar.classList.remove('open')`.
  6. for every `[data-nav]`: `active = href === '/' ? path === '/' : path.startsWith(href)` → `classList.toggle('active', …)`. Hence `/library/album/x` → Library, `/playlists/3` → Playlists; `/artist/…`, `/album/…`, `/tag/…`, `/settings`, `/storage` highlight nothing.
  7. `view = document.createElement('div')`; `qs('#view').replaceChildren(view)` (fresh container per navigation: a late async page writes into a detached node).
  8. first matching route: (`adminOnly` third tuple slot exists but no route sets it → would `break` to not-found); `window.scrollTo(0, 0)`; `await fn(view, { params: m.slice(1), query, user: session.user, onLeave: (f) => (view.isConnected ? cleanup.push(f) : f()), navigate })`; on throw: if `!view.isConnected` return silently, else `console.error(e)` and `view.innerHTML = `<div class="error-box">${esc(e.message || e)}</div>``; return.
  9. no match: `view.innerHTML = `<div class="empty"><h2>Page not found</h2><p class="muted"><a href="#/">Go home</a></p></div>``.

### 8.4 DOM contract
**`avatarHtml(u, size = 32)`**: `style = size !== 32 ? ` style="width:${size}px;height:${size}px;font-size:${Math.round(size * 0.42)}px"` : ''`;
`u.avatar_at` → `<img class="avatar" src="/api/users/${u.id}/avatar?v=${Math.round(u.avatar_at)}" alt=""${style}>`
else → `<div class="avatar"${style}>${esc((u.username || '?')[0])}</div>` (CSS upper-cases it).

**`navLink(href, ic, label, extra = '')`** → `<a href="#${href}" data-nav="${href}">${ic}<span>${label}</span>${extra}</a>`.

**`renderShell()`** – `u = session.user`, `admin = ['admin','owner'].includes(u.role)`, `appName = esc(info.app_name || 'Encore')`, `document.title = info.app_name || 'Encore'`; `#app.innerHTML =`
```html
<div class="shell">
  <aside class="sidebar" id="sidebar">
    <a class="brand" href="#/">${appearance.logo()}${appName}</a>
    <nav class="nav">
      <a href="#/" data-nav="/">${icon.home}<span>Home</span></a>
      <a href="#/search" data-nav="/search">${icon.search}<span>Discover</span></a>
      <a href="#/for-you" data-nav="/for-you">${icon.spark}<span>For you</span></a>
      <a href="#/library" data-nav="/library">${icon.library}<span>Library</span></a>
      <a href="#/requests" data-nav="/requests">${icon.inbox}<span>Requests</span><span class="badge warn hidden" id="pending-badge"></span></a>
      <a href="#/playlists" data-nav="/playlists">${icon.list}<span>Playlists</span></a>
      <a href="#/download" data-nav="/download">${icon.download}<span>Downloader</span></a>
      <a href="#/leaderboard" data-nav="/leaderboard">${icon.activity}<span>Leaderboard</span></a>
    </nav>
    <div id="storage-mini" style="flex:none"></div>
    <div class="sidebar-foot" style="margin-top:0">
      <a href="#/settings?s=account" class="row grow" style="min-width:0">
        ${avatarHtml(u)}
        <div class="grow"><div class="ellipsis" style="font-weight:650">${esc(u.username)}</div><div class="small faint">${u.role === 'owner' ? 'Owner' : admin ? 'Admin' : 'Member'}</div></div>
      </a>
      <a class="btn ghost icon" href="#/settings" title="Settings" aria-label="Settings">${icon.settings}</a>
      <button class="btn ghost icon" id="logout-btn" title="Sign out">${icon.logout}</button>
    </div>
  </aside>
  <main class="main">
    <button class="btn ghost icon mobile-only" id="menu-btn" style="margin:-6px 0 10px -6px">${icon.menu}</button>
    <div id="view"></div>
  </main>
</div>
```
(The literal has leading whitespace/newlines between elements; `.nav` is flex so it is irrelevant.) The nav is the same for every role; admin-only pages live inside Settings.

**`#storage-mini`** is filled by `pages/storage.js sidebarMeter(el)` (`GET /api/library/storage`): with `disk` →
```html
<a href="#/storage" class="storage-mini" title="Storage">
  <div class="row between small"><span>Storage</span><span class="faint">${fmt.size(disk.free)} free</span></div>
  <div class="progress" style="height:6px;margin-top:6px"><i style="width:${pct}%;${pct > 90 ? 'background:var(--err)' : ''}"></i></div>
  <div class="faint" style="font-size:11px;margin-top:4px">Music ${fmt.size(library.size)} · ${fmt.size(disk.total)} disk</div></a>
```
(`pct = disk.used / disk.total * 100`, unrounded); without `disk` → `<a href="#/storage" class="storage-mini"><span>Library ${fmt.size(library.size)}</span></a>`; on error → `''`.

**Pending badge**: `#pending-badge.textContent = rows.length`, `hidden` toggled when `rows.length === 0`.

**Error / not-found** views: see §8.3 (8, 9).

**Connection banner** `connBanner(on, text = 'Not connected – check your internet or server')`: `on=false` → remove `#conn-banner`. `on=true` → create `div#conn-banner` (appended to body) if missing; `innerHTML = `${icon.x}<span>${esc(text)}</span>${window.EncoreNative?.openOffline ? '<button class="btn sm" id="go-offline">Offline music</button>' : ''}``; `#go-offline` click → `window.EncoreNative.openOffline()`. Texts: default (offline event) and `The Encore server isn't answering` (server-down).

### 8.5 Events and interactions
| Element / source | Event | Effect |
|---|---|---|
| `#logout-btn` | click | `await api.post('/auth/logout')`; `location.reload()` |
| `#menu-btn` | click | `stopPropagation()`; `#sidebar.classList.toggle('open')` |
| `document` | click | if `#sidebar.open` and the click target is outside the sidebar → remove `open` (registered per `renderShell` call) |
| `window` | `hashchange` | `route()` |
| `window` | `encore:unauthorized` | see §0.3 (toast + hash `#/login` + reload) |
| `window` | `encore:requests-changed` | `refreshPendingBadge()` |
| `window` | `offline` / `online` | banner on / off |
| `window` | `encore:server-down` / `encore:server-up` | banner `The Encore server isn't answering` / off |
| `window` | `encore-native` | toast from the native host |
| timer | 30 s | `refreshPendingBadge()` (admins only; silently ignores errors) |
| timer | 5 min | storage meter refresh |
| route change | | sidebar closed, `#modal-root` emptied, `scrollTo(0,0)` before rendering, page `onLeave` cleanups run |

Toasts raised by the shell: `Your session expired – please sign in again` (err); native-relayed texts (kind from host, default ok).

### 8.6 API calls
| Call | When | Response use | Errors |
|---|---|---|---|
| `GET /auth/info` | boot | `session.info` (`app_name`, `signup_mode`, `features`, `email_login` are read by shell/auth/settings) | → `{}` |
| `GET /auth/me` | boot, `refreshUser` | `session.user` | boot → `null` (signed out); `refreshUser` lets it throw |
| `GET /requests?status=pending&limit=500` | badge refresh (admin/owner only) | array length only | ignored |
| `GET /library/storage` | sidebar meter (via storage.js) | `disk{used,free,total}\|null`, `library{size,tracks,albums}` | meter cleared |
| `POST /auth/logout` | sign-out button | none; then reload | unhandled (reload not reached on failure) |

### 8.7 Browser APIs
`location.hash` / `history.replaceState` / `HashChangeEvent`; `navigator.serviceWorker.register('/sw.js')` (https or localhost only); `window.scrollTo(0,0)`; `document.title`; `setInterval`; `window.EncoreNative.openOffline` (optional).

### 8.8 Dependencies
Imports: `appearance.{apply, adoptAccount, logo}`, `remote.start`, `api`, `player.mount`, `ui.{esc, icon, qs, toast}`, `pages/auth.render`, `pages/home.render`, `pages/discover.{renderSearch, renderArtist, renderArtistByName, renderAlbum}`, `pages/library.{render, renderAlbum}`, `pages/requests.render`, `pages/download.render`, `pages/settings.render`, `pages/playlists.{render, renderPlaylist}`, `pages/foryou.{render, renderTag}`, `pages/storage.{render, sidebarMeter}`, `pages/leaderboard.render`.
Page renderer contract: `async fn(view: HTMLElement, ctx: { params: string[], query: Record<string,string>, user, onLeave(fn), navigate(path) })`. Auth renderer: `render(root, { path, query, session })`.

`remote.start(session)` returns immediately unless `session.info.features` includes `'remote'`; it then sets `window.encoreRemote`, restores a saved link (`encore.link`) and starts a heartbeat loop (`POST /remote/heartbeat` every 2 s, 6 s when the tab is hidden and idle). `player.mount()` is idempotent (`#player` exists → return), appends the audio elements, `#player`, `#np-full`, registers MediaSession handlers and the global keyboard shortcuts (Space / ←→ seek ±10 s when focus is not in a form control, Escape closes the full-screen player).

### 8.9 Gotchas
- `renderShell()` is re-run by `refreshUser()` and whenever `#view` is missing; each run re-registers the document click listener and a new 5-minute interval → register once in the rewrite.
- `route()` is not awaited by `boot`/`refreshUser`; two overlapping `route()` calls are safe thanks to the fresh `view` container per call.
- The signed-out flow never renders the shell; after a successful login the auth page sets `location.hash = '#/'` and `location.reload()` (full reload rather than in-app state change).
- `document.title` is `app_name` unescaped (it is a text property).
- `esc()` is applied to `u.username`, `app_name`, error messages, banner text; `role` label is a fixed word.
- The pending badge is only populated for `admin`/`owner`; its element exists for everyone (hidden).

---

## 9. `web/css/app.css` – selector inventory

### 9.1 Variables (`:root`) and theme overrides
| Variable | dark (default) | `[data-theme="oled"]` | `[data-theme="light"]` | Set by JS |
|---|---|---|---|---|
| `--bg` | `#0b0b12` | `#000` | `#f6f6fb` | |
| `--bg-2` | `#12121c` | `#000` | `#ffffff` | |
| `--panel` | `#161624` | `#0b0b0d` | `#ffffff` | |
| `--panel-2` | `#1d1d2e` | `#17171b` | `#f0f0f7` | |
| `--border` | `#2a2a40` | `#232329` | `#e2e2ee` | |
| `--text` | `#ececf4` | – | `#16161f` | |
| `--muted` | `#9a9ab3` | `#9a9aa8` | `#5d5d75` | |
| `--faint` | `#6b6b85` | `#66666f` | `#8d8da5` | |
| `--accent` / `--accent-2` | `#8b5cf6` / `#ec4899` | | | appearance.apply (inline on `html`) |
| `--accent-rgb` / `--accent-2-rgb` | `139, 92, 246` / `236, 72, 153` | | | appearance.apply |
| `--on-accent` | `#fff` | | | appearance.apply (`#16161f` for light accents) |
| `--grad` | `linear-gradient(135deg, var(--accent), var(--accent-2))` | | | |
| `--content` | `1180px` | | | `:root[data-width="wide"] { --content: none }` |
| `--ok/--warn/--err/--info` | `#22c55e/#f59e0b/#ef4444/#38bdf8` | | | |
| `--radius` | `14px` | | | |
| `--shadow` | `0 10px 30px rgba(0,0,0,.35)` | `none` | `0 10px 30px rgba(20,20,50,.08)` | |
| `--sidebar` | `236px` | | | |
| `--player` | `0px` | | | `body.has-player { --player: var(--player-h) }`; `112px` ≤960px |
| `--player-h` | `76px` | | | |
| `color-scheme` | dark | | light | |
| `--np-bg` | (unset) | | | player.js sets on `.np-full` for the blurred backdrop |
| `--p` | `0%` on `#seek`/`#fseek` | | | player.js: played fraction for the range track gradient |

`html[data-theme]`, `html[data-bg]`, `html[data-width]` are the only root attributes; `:root[data-bg="fade"] body` gets two accent radial gradients (`rgba(var(--accent-rgb), .16)`, `rgba(var(--accent-2-rgb), .10)`) with `background-attachment: fixed`, and `:root[data-bg="fade"] .sidebar` becomes translucent (`color-mix(... 82%)` + `backdrop-filter: blur(12px)`).

### 9.2 Base and utilities
`*` border-box; `html, body` margin 0 height 100%; `body` bg/text vars, `font: 14px/1.5 Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif`, antialiased; `a` inherits colour, no underline; form controls inherit font/colour; `img` block; `h1/h2/h3` margin 0, lh 1.2, ls −.01em; `h1` 28px/750, `h2` 19px/700, `h3` 15px/650.
Utilities: `.muted`, `.faint`, `.small` (12px), `.mono`, `.ellipsis`, `.hidden` (`display:none !important`), `.grow` (`flex:1; min-width:0`), `.row` (flex, center, gap 10), `.row.wrap`, `.stack` (column, gap 12), `.between`, `.gap-lg` (20), `.mt` (16), `.mt-lg` (28), `.mobile-only` (hidden ≥901px, `inline-flex !important` ≤900px), `.hide-sm` (hidden ≤760), `.show-sm` (hidden; `inline !important` ≤760).

### 9.3 Layout (shell) – depends on the exact nesting of §8.4
| Selector | Role |
|---|---|
| `.shell` | `grid; grid-template-columns: var(--sidebar) 1fr; min-height:100vh` → `1fr` ≤900px |
| `.sidebar` | sticky top 0, height 100vh, `bg-2`, right border, flex column, `padding: 18px 12px calc(var(--player) + 12px)`, `padding-top: max(18px, env(safe-area-inset-top))`. ≤900px: `position:fixed; z-index:60; width:260px; transform:translateX(-100%); transition .2s; box-shadow` and `.sidebar.open { transform:none }` |
| `.brand` | flex, gap 10, `padding: 4px 10px 18px`, 800/18px; `.brand img, .brand svg.logo` 30×30 flex none |
| `.nav` | flex column, gap 2, `flex:1 1 auto; min-height:0; overflow-y:auto; scrollbar-width:thin`; `.nav a` flex none, `display:flex; gap 12; padding 9px 12px; radius 10; color muted; weight 550`; hover → `panel-2` + text; `.nav a.active` same + `box-shadow: inset 3px 0 0 var(--accent)`; `.nav a svg` 18px; `.nav .badge { margin-left:auto }` |
| `.nav-label` | 11px uppercase label (used in settings nav: `.settings-nav .nav-label { margin: 12px 12px 4px }`) |
| `.sidebar-foot` | `margin-top:auto; flex; gap 10 → overridden to 2; padding 10; border-top`; `.sidebar-foot .btn.icon { padding:6px }` |
| `.avatar` | 32×32 circle, `background: var(--grad); color: var(--on-accent); grid center; 700; text-transform: uppercase`; `img.avatar` object-fit cover, bg panel-2 |
| `.storage-mini` | block `<a>`; `margin: 8px 10px; padding: 10px 12px; border 1px; radius 10; color text`; hover panel-2 |
| `.main` | `min-width:0; overflow-x:clip; padding: 26px 32px calc(var(--player) + 40px)`; `padding-top: max(16px, env(safe-area-inset-top))` (later rule wins); ≤760px `padding: 16px 16px calc(var(--player) + 30px)` (then top again overridden by the safe-area rule) |
| `.main > #view` | `max-width: var(--content); margin: 0 auto` – `#view` **must** be a direct child of `.main` |
| `.topbar`, `.topbar .search` | flex row, mb 22; search `flex:1; max-width:560px` |
| `#conn-banner` | fixed top bar, z 200, flex gap 10, `padding: 8px 14px` (+safe-area top), `background:#2b1d05; color:#fcd34d; 14px/600; border-bottom 1px #4a3410`; `svg` 16px; `span { flex:1 }` |

### 9.4 Appearance picker (Settings → Appearance, DOM produced by settings.js)
`.swatches` (wrap, gap 10); `.swatch` 44px circle, 2px transparent border, `.swatch.on` border text-colour + `0 0 0 3px var(--bg)` ring; `.swatch input` invisible overlay; `.seg` inline-flex pill group (`.seg button`, `.seg button.on`); `.presets` auto-fill grid minmax(130px); `.preset` column card, 2px border, `.preset.on` accent border, `.preset i` 42px preview block; `.custom-colour` row, `input[type=color]` 44×36, `input.input` 110px monospace.

### 9.5 Controls
| Family | Rules |
|---|---|
| `.btn` | inline-flex center, gap 8, `padding 9px 16px`, radius 10, 1px border, `panel-2` bg, 600, nowrap; hover border accent; active scale .97; `:disabled` opacity .5; `.primary` (`--grad`, transparent border, `--on-accent`, hover brightness 1.08); `.danger` (text err; hover border err); `.ghost` (transparent; hover panel-2); `.sm` (`6px 11px`, 13px, radius 8); `.icon` (padding 8); `.btn svg` 16px |
| `.input`, `select.input`, `textarea.input` | width 100%, `padding 10px 13px`, radius 10, border, `bg-2`; focus accent border + `0 0 0 3px rgba(var(--accent-rgb), .2)`; textarea min-height 90 vertical resize; `:disabled` opacity .65 |
| `.search` | relative; `.search svg` absolute left 13 centred 17px faint; `.search .input` padding-left 40 |
| `label.field` | column gap 6; `> span` 13px/600 muted; `small` faint 12px. `div.field` same + `small` block |
| `.switch` | inline-flex; `input` hidden; `i` 38×22 pill (`border` bg), `::after` 16px white knob; `input:checked + i` accent, knob `translateX(16px)` – **requires `<input>` immediately followed by `<i>`** |
| `.tabs` | flex gap 4 padding 4 panel, radius 12, fit-content, `overflow-x:auto`; `.tabs button` 7px 14px radius 8 muted 600; `.active` panel-2 + text + shadow |
| `.badge` | inline-flex gap 5, `2px 9px`, pill, 11.5px/650, panel-2/muted/border; `.ok/.warn/.err/.info` tinted (colour, border rgba .35, bg rgba .1); `.accent` (`color-mix(accent 55%, text)`, rgba accent .4/.14) |
| `.dot` | 8px circle faint; `.ok` (glow), `.warn`, `.err` |
| `.fab` | 40px circle, `--grad`, `--on-accent`, accent shadow; `.fab svg` 18; `.fab.light` dark translucent + blur |
| `.play-btn` | 40px circle, bg text / colour bg; `.lg` 64px (svg 26) |
| `.play-thumb` | 44px borderless button; `img/.ph` cover; `i` overlay (rgba .45) shown on hover/focus-visible; `i svg` 18 |
| `.dir-btn` | min-width 38 centred |
| `.kbd` | monospace 11px bordered key |
| `.eye-btn` | 34px inline-grid button muted, radius 8; hover text + panel-2; svg 18 |
| `.pw-wrap` | relative block; `> input` width 100% `padding-right 42px`; `> .eye-btn` absolute right 4 centred; `.row > .pw-wrap:not(.grow) { flex: 0 1 auto }`; `.pw-wrap.secret > input` padding-right 78; `.pw-wrap.secret > .eye-btn[data-reveal]` right 40 (settings.js) |

### 9.6 Cards, stats, hero, grids, tiles
`.card` (panel, border, `--radius`, padding 18); `.card.flat` (padding 0, overflow hidden); `.card.linked` accent border; `.section-head` (flex between, `margin: 28px 0 14px`, `:first-child` mt 0); `.stats` auto-fill minmax(170px) gap 14; `.stat` card-like, `b` block 26px/750, `span` muted 13px; `.hero` (relative, radius 20, padding 30 → 22 ≤760, two accent radial gradients over panel, border; `h1` 30px; `.hero .search` max 640 mt 18; `.hero .search .input` `14px 16px 14px 44px`, 15px, radius 14).
`.grid` auto-fill minmax(168px) gap 18 (≤760: 140px / 14); `.art-grid` minmax(140px).
`.tile` relative pointer min-width 0; `.tile .art` square, radius 12, hidden overflow, panel-2, shadow, hover `translateY(-3px)`; `.tile .art img` cover; `.tile .art .ph, .ph` absolute inset 0 grid-centred gradient placeholder 34px/800 faint; `.tile .art .overlay` bottom-right actions (hidden until hover, `translateY(6px)`); `.tile .art .corner` top-left 8px; `.tile .t` (title, mt 9, 650) / `.tile .s` (muted 13px); `.round .art` circle; `.tile .art .overlay .fab` accent shadow.
`.art-pick` borderless padded button, hover accent border, `.busy` half-opacity no-pointer.

### 9.7 Lists and tables
`.list` column; `.list-item` flex gap 14 `11px 16px` bottom border (none on last), hover panel-2; `.thumb` 48px radius 8 panel-2 relative hidden (`.lg` 64); `.table-wrap` overflow-x auto; `table.table` 100% collapse; `th` left 12px uppercase faint `10px 14px` bottom border nowrap; `td` `10px 14px` bottom border middle; last row no border; `tr:hover td` panel-2; `td.num` 40px faint right; `.tracklist .playing td` / `.tracklist tr.playing td` accent; `.tracklist tbody tr` pointer; `.progress` 6px pill panel-2, `> i` block full height `--grad` width-transition .4s (used by storage meter).
≤760px: `.table-wrap` overflow-x hidden, `table.table` fixed layout, cells `10px 8px`, first column 60px, last column 44px right-aligned, `.hide-sm` cells hidden, ellipsis on `td`.
Picker rows: `.pick-row` (flex gap 12 `12px 4px` bottom border), `.pt` 600 wrap-anywhere, `.ps` 12px muted 2-line clamp, `.pm` meta chips row, `.pick-row .btn` flex none.

### 9.8 Detail header, bio, chips
`.detail-head` flex gap 28 align end mb 24 (column/start ≤760); `.detail-head .cover` 220px radius 16 panel-2 shadow relative (160px ≤760), `img` cover; `.detail-head.artist .cover` circle; `.kicker` 12px uppercase ls .1em muted 700; `.detail-head h1` 40px `6px 0 10px` (28px ≤760); `.bio` padding `16px 18px 10px`, `p` lh 1.6 max 80ch; `.clamp` 3-line clamp; `.chips` wrap gap 6.

### 9.9 Auth
`.auth-wrap` min-height 100vh grid-centred, padding 20, accent radial gradients over `--bg`; `.auth-card` max 400px panel border radius 20 padding 32 shadow; `.auth-card .brand` centred, `padding 0 0 6px`, 24px; `.auth-card .brand img` 40px; `.auth-card form` column gap 14 mt 22; `.error-box` red tint box 13px (`#fca5a5`; light theme `#b91c1c`).

### 9.10 Modal, toast, menus
`.modal-bg` fixed inset 0 `rgba(5,5,10,.6)` blur 4, flex centre, z 100, padding 20, `fade .15s`; ≤760: padding 8, `align-items:flex-end` (sheet). `.modal` panel border radius 18, width 100% max 560 (`.wide` 980), max-height 88vh (94vh ≤760, radius `18px 18px 12px 12px`), column, `pop .18s`; `.modal-head` flex between `18px 22px` bottom border (`14px 16px` ≤760, `h2` 17px); `.modal-head h2` ellipsis; `.modal-body` `20px 22px` auto-scroll, no x-overflow (`14px 16px` ≤760); `.modal-foot` flex end gap 10 `14px 22px` top border. Keyframes `fade` (opacity from 0) and `pop` (opacity 0, `scale(.97) translateY(6px)`).
`#toasts` fixed `right 18px; bottom calc(var(--player) + 18px)`, z 200, column gap 8; `.toast` panel border, `border-left 4px var(--accent)`, `12px 16px`, radius 12, shadow, max 380, `pop .2s`; `.toast.ok` / `.toast.err` left border ok/err.
`.menu-bg` fixed inset 0 z 140 (≤760 `rgba(5,5,10,.5)`); `.menu-pop` fixed z 141, min 220 max 320, panel border radius 12 shadow padding 6, `pop .12s`; `button, a` full-width flex gap 10 `9px 10px` radius 8 text left, hover panel-2; `.danger` err; `svg` 16 opacity .85; `hr` 1px rule `4px 0`. ≤760: bottom sheet `left:8px !important; right:8px; bottom:8px; top:auto !important; max-width:none; radius 16; padding 8`, items `13px 12px` 15px.

### 9.11 Player bar (DOM from player.js; listed for completeness)
`body.has-player { --player: var(--player-h) }` (toggled by player.js when a queue exists). `.player` fixed bottom, height `--player-h`, z 50, translucent `bg-2` 90% + blur 16, top border, `grid-template-columns: minmax(0,1fr) minmax(0,1.4fr) minmax(0,1fr)`, gap 18, `padding 0 20px`, `padding-bottom: max(10px, env(safe-area-inset-bottom))`. Children: `.now` (flex gap 12, pointer, radius 10, hover outlines `.thumb` accent; `.thumb` 48 radius 8), `.ctrls` (column centre gap 4; `.row` gap 6), `.seek` (flex gap 10 100% 11px faint; `span` min 34 centred tabular), `input[type=range]` flex 1 accent-color, `#seek` custom range (height 22, transparent, track 5px gradient `accent var(--p) / border`, 15px thumb), `.right` (flex end gap 8; `input` max 100).
≤960px: `--player: 112px`, `.player` height 112, `grid-template-columns: minmax(0,1fr) auto auto; grid-template-rows: auto 1fr; gap 0 8px; padding 6px 12px 8px`; `.ctrls { display: contents }`; `.seek` spans row 1 (`#seek` 32px tall, 20px thumb); `.now` col 1 row 2; `.ctrls .row` col 2 row 2; `.right` col 3 row 2; `.right input/.faint/.badge` hidden; `#p-prev` hidden. ≤760: `.player .right .btn#p-queue { display:inline-flex }`.
Full-screen player: `.np-full` fixed inset 0 z 120 column `--bg` `fade`; `.np-backdrop` blurred `--np-bg`; `.np-full > *:not(.np-backdrop)` relative; `body.np-open` overflow hidden; `.np-top` grid `44px 1fr 44px` (`40px 1fr 8px` ≤760) padding `14px 18px` (+safe-area top); `.np-tabs` centred; `.np-body` grid `1.1fr 1fr` gap 32 `8px 40px 28px`, panes toggled by `.pane-now|.pane-lyrics|.pane-queue` (desktop: now+queue visible, lyrics replaces queue; ≤760: single pane, block layout, scroll); `.np-main`, `.np-art` (`min(52vh,100%)` square radius 16, `min(82vw,46vh)` ≤760), `.np-meta` (`h2` 26px → 21px), `.np-seek` (`min(560px,100%)`; `#fseek` 28px tall, 6px track, 18px thumb), `.np-ctrls` (gap 14; `.btn.on` accent; `sup` 9px), `.np-vol` (`min(260px,100%)`), `.np-lyrics` (`padding 30vh 8px`, mask gradient; `p` 24px/700 muted .55 pointer, `.on` full; `.np-plain` pre-wrap 18px; `.np-empty`), `.np-queue h3`, `.np-item` (flex gap 10 padding 8 radius 10 pointer, hover; `.thumb` 42).
Remote link: `.linked-to` inline-flex accent 600, `.linked-to svg, #p-unlink svg` 14px.

### 9.12 Misc, settings, responsive
`.empty` centred `50px 20px` muted, `svg` 46px faint mb 10; `.skeleton` shimmer gradient radius 12 (`shimmer 1.2s`); `.spinner` 18px ring accent top (`spin .7s`); `.code` monospace box 13px break-all; `.diff-old` strike faint, `.diff-new` ok; `.status-pill` inline-flex gap 6; `.test-result` flex `11px 14px` radius 10 mb 16 panel-2 550 (`.ok`/`.err` tints; `svg` 18 mt 1; `span` break-word); `small.src` muted, `.env` info, `a` accent; `.diag-raw` pre-wrap max 220 scroll.
Settings: `.settings-grid` `220px minmax(0,1fr)` gap 24 start (1fr ≤960, `.settings-nav` hidden, `.settings-select { display:block !important; mb 16 }`); `.settings-grid > #panel` min-width 0; `.settings-nav` sticky top 20 column gap 2 `max-height: calc(100vh - var(--player) - 40px)` auto-scroll; `.settings-nav button` left-aligned ghost `9px 12px` radius 9 muted 600, `.active` panel-2 text; `.settings-select` hidden by default; `.settings-card + .settings-card` mt 16; `details.settings-card > summary` no marker pointer, `.chev svg` 18 rotates 180° when `[open]`; `.form-grid` 2 columns gap 16 (1 column ≤760), `.full` spans; `.field .switch` align start lh 1.35; `.embedded h1, #panel h1` 22px.
Breakpoints used: `max-width: 960px` (settings grid, phone player), `900px` (sidebar drawer, `.mobile-only`), `760px` (content padding, grid, detail head, forms, hero, hide/show-sm, modal sheet, menu sheet, tables, full-screen player).
Safe areas: `.main`, `.np-top`, `.sidebar` top insets; `.player` bottom inset; `#conn-banner` top inset.
Cover fill rule: `.thumb > img, .tile .art > img, .np-art > img, .play-thumb > img, .detail-head .cover > img { position:absolute; inset:0; width/height 100%; object-fit:cover }` – images must be **direct children** of those frames, with the `.ph` sibling before them (see `ui.art`).

### 9.13 Classes / attributes toggled by JS (must be driven identically)
`html[data-theme=dark|oled|light]`, `html[data-bg=fade|solid]`, `html[data-width=comfortable|wide]`, inline `--accent*`/`--on-accent` on `html`; `body.has-player`, `body.np-open`; `.sidebar.open`; `.nav a.active`; `.hidden` (`#pending-badge`, `#np-full`, `#auth-err`, forms…); `.tabs button.active`; `.settings-nav button.active`; `.swatch.on`, `.preset.on`, `.seg button.on`; `.np-body.pane-*`, `.np-ctrls .btn.on`, `.np-lyrics p.on`; `.tracklist tr.playing`; `.art-pick.busy`; `.card.linked`; `.toast.ok|.err`; `.test-result.ok|.err`; `.modal.wide`; `#seek`/`#fseek` `--p`; `.np-full` `--np-bg`; `input[data-eye]`, `[data-secret]`, `[data-shown]`, `[data-reveal]`; `.menu-pop` inline `left/top`.

---

## 10. Rewrite notes (plan input for the Leptos shell)

1. **Entry**: `main()` → `appearance::apply(current())` synchronously, then mount on `#app`. Keep `#toasts` and `#modal-root` as static siblings (index.html already has them); mount toasts/modals there via portals or direct DOM appends with the exact markup of §7.3/§7.4.
2. **Session**: `RwSignal<Option<User>>` + `info`; fetch `/auth/info` then `/auth/me` exactly as §0.2; expose `window.encoreSession` (or a `js_sys` shim) only if legacy pages still run – once every page is Rust, `library.rs` can read the signal instead.
3. **Router**: hand-rolled hash router (regex table of §0.6, `hashchange` listener, `redirect` with `replaceState` + synthetic `HashChangeEvent`). Keep `onLeave` semantics (cleanup on navigation, fresh `#view > div` per route) and the active-link rule (`startsWith`).
4. **Shell component**: render §8.4 verbatim; register the outside-click listener and the two intervals **once**; `refreshUser` re-renders only the sidebar-foot.
5. **api module**: `request()` as §5.2 with `wasm-bindgen-futures`; dispatch the three custom events so the banner/unauthorized logic stays decoupled; expose `ApiError{status, message, data: serde_json::Value}`.
6. **ui module**: `esc` (only needed where raw HTML strings are still built), icons as `&'static str` constants, `toast`, `modal`, `confirm_dialog` (resolve-false-on-removal via `MutationObserver` or by routing every close path through one function), `fmt`, `art`, `status_badge`, `pop_menu` (positioning rule §7.9), password eye wrapper as an explicit component.
7. **Service worker / manifest / share.html / favicon**: copy unchanged; the `/next/` build registers the same `/sw.js` only while served from `/`; when `/next/` replaces `/`, nothing changes.
8. **Parity checks**: diff the serialized DOM of `#app` (signed-out and signed-in), `#player`, a toast, a modal and a pop menu between `/` and `/next/`; compare `localStorage`/`sessionStorage` keys after the same actions; replay the API log.
