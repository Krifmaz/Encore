// Encore service worker: the website opens and runs without the server once it has been loaded here.
//
// - The app shell (the page, its hashed script and WebAssembly, the stylesheet, the icons, the manifest and
//   the share page) is saved when the worker installs.
// - Hashed files (encore-web-<hash>.js, encore-web-<hash>_bg.wasm) never change under their name: they come
//   from the cache first and from the network only when missing.
// - The rest of the shell is network-first (always fresh when the server answers) with the saved copy as the
//   fallback; a page load (navigation) that fails falls back to the saved /index.html. An answer that is not
//   Encore's counts as failed: Encore marks every answer with `x-encore`, and an error page without it comes
//   from a proxy in front of a server that is down (Caddy and nginx 502, Traefik 404, Cloudflare 52x or 530).
//   When a saved copy exists the network gets WAIT_MS to answer, so a server whose address drops packets
//   does not hold the page for the connect timeout (a minute or more); the late answer still refreshes the
//   saved copy.
// - /api/ (and so every stream and cover), /sw.js itself, share pages (/s/…), the /next alias and other sites
//   are never touched: the browser handles them as if there were no worker.
// - A new worker does not take over on its own: it waits, the page shows "A new version of Encore is ready"
//   and, when the user presses Reload, posts {type: 'skip-waiting'}. A page that posts {type: 'version?'} is
//   answered with {type: 'encore:version', cache}.
// - Activating deletes the shells of earlier builds (encore-shell-*) and never the saved songs
//   (encore-offline-*), which belong to the page.
//
// This file is a template: hooks/sw.sh (a Trunk post_build hook) fills in the cache name (made from the
// build's content hash, so every build gets a new one) and the shell list (the fixed files, the icons and the
// hashed .js and .wasm of this build), and writes the result to dist/sw.js.
const CACHE = '__CACHE__';
const SHELL = [__SHELL__];
const SHELL_PREFIX = 'encore-shell-';
// the server's rule for hashed files (server/src/main.rs `hashed_asset`): at the root, a stem that ends in
// -<12 or more hex digits>, optionally followed by _bg (trunk drops the hash's leading zeros)
const HASHED = /^\/[^/]+-[0-9a-f]{12,}(_bg)?\.[A-Za-z0-9]+$/;
const SHELL_PATHS = new Set(SHELL);
// how long the network gets before the saved copy answers (only when there is one)
const WAIT_MS = 4000;

function isShell(path) {
  return SHELL_PATHS.has(path) || path.startsWith('/icons/') || path.startsWith('/css/');
}

function untouched(url) {
  if (url.origin !== self.location.origin) return true;
  const p = url.pathname;
  return p.startsWith('/api/') || p === '/sw.js' || p.startsWith('/s/') || p === '/next' || p.startsWith('/next/');
}

// a missing file gets the server's index.html with 200 (the single-page fallback): that page is never kept
// under another file's name
function keep(res, path) {
  if (!res || !res.ok || res.type !== 'basic' || res.redirected) return false;
  const html = (res.headers.get('content-type') || '').includes('text/html');
  return !html || path === '/' || path.endsWith('.html');
}

async function put(req, res) {
  try {
    const c = await caches.open(CACHE);
    await c.put(req, res);
  } catch (e) { /* a full disk or a private window: the page still works from the network */ }
}

async function fromCache(req) {
  const c = await caches.open(CACHE);
  return c.match(req, { ignoreVary: true });
}

async function cacheFirst(req) {
  const hit = await fromCache(req);
  if (hit) return hit;
  const res = await fetch(req);
  if (keep(res, new URL(req.url).pathname)) put(req, res.clone());
  return res;
}

// an answer that is not Encore's: the error page of a proxy in front of a server that is down
function foreign(res) {
  return res.type !== 'opaqueredirect' && !res.headers.has('x-encore') && (res.status === 404 || res.status >= 500);
}

// fetch(req); a good answer is saved under `as` (when given). The saving keeps the worker alive (waitUntil),
// also when the page was answered from the cache meanwhile.
function fetchKeeping(e, req, path, as) {
  const net = fetch(req);
  e.waitUntil(net.then(res => (as && keep(res, path) ? put(as, res.clone()) : undefined)).catch(() => {}));
  return net;
}

// the network's answer when it is Encore's and comes within WAIT_MS, else the saved copy; with nothing saved,
// whatever the network answers (or its error)
async function freshOrSaved(net, saved) {
  const hit = await saved();
  if (!hit) return net;
  let timer;
  const late = new Promise(r => { timer = setTimeout(() => r(null), WAIT_MS); });
  try {
    const res = await Promise.race([net, late]);
    if (res && !foreign(res)) return res;
  } catch (err) {
    // no network: the saved copy
  } finally {
    clearTimeout(timer);
  }
  return hit;
}

function networkFirst(e, req, path) {
  const net = fetchKeeping(e, req, path, isShell(path) ? req : null);
  return freshOrSaved(net, () => fromCache(req));
}

function navigate(e, req, path) {
  const net = fetchKeeping(e, req, path, path === '/' || path === '/index.html' ? path : null);
  return freshOrSaved(net, async () => (await fromCache(req)) || (await fromCache('/index.html')) || (await fromCache('/')));
}

self.addEventListener('install', (e) => {
  // no skipWaiting(): a new worker waits until the page asks for it (the Reload button)
  e.waitUntil(caches.open(CACHE).then(c => c.addAll(SHELL.map(u => new Request(u, { cache: 'reload' })))));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys()
    .then(keys => Promise.all(keys.filter(k => k.startsWith(SHELL_PREFIX) && k !== CACHE).map(k => caches.delete(k))))
    .then(() => self.clients.claim()));
});

self.addEventListener('message', (e) => {
  const d = e.data;
  if (!d || typeof d !== 'object') return;
  if (d.type === 'skip-waiting') {
    self.skipWaiting();
  } else if (d.type === 'version?') {
    const answer = { type: 'encore:version', cache: CACHE };
    if (e.ports && e.ports[0]) e.ports[0].postMessage(answer);
    else if (e.source) e.source.postMessage(answer);
  }
});

self.addEventListener('fetch', (e) => {
  const req = e.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  if (untouched(url)) return;
  const path = url.pathname;
  if (req.mode === 'navigate') {
    e.respondWith(navigate(e, req, path));
  } else if (HASHED.test(path)) {
    e.respondWith(cacheFirst(req));
  } else if (isShell(path)) {
    e.respondWith(networkFirst(e, req, path));
  }
});
