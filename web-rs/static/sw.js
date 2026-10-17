// Encore service worker: makes the web app installable and opens instantly.
// The app shell is served network-first (always fresh when the server is reachable) with a cached
// fallback; API calls, streams and the built-in services are never cached.
//
// This file is a template: hooks/sw.sh (a Trunk post_build hook) fills in the cache name (made from the
// build's content hash, so every build retires the shell of the one before) and the shell list (the fixed
// files plus the hashed .js and .wasm of this build), and writes the result to dist/sw.js.
const CACHE = '__CACHE__';
const SHELL = [__SHELL__];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then(c => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys().then(keys => Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k)))).then(() => self.clients.claim()));
});

self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== 'GET' || url.origin !== location.origin || url.pathname.startsWith('/api/')) return;
  e.respondWith(fetch(e.request).then(res => {
    if (res.ok && (res.type === 'basic' || res.type === 'default')) {
      const copy = res.clone();
      caches.open(CACHE).then(c => c.put(e.request, copy));
    }
    return res;
  }).catch(() => caches.match(e.request).then(hit => hit || caches.match('/index.html'))));
});
