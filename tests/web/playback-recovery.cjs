// Playwright check for the web player keeping a song going when its stream breaks off (0.24.1 hotfix), run against
// the Rust/WASM UI (web-rs):
//   cd web-rs && trunk build --release
//   NODE_PATH=/opt/node-tools/node_modules node tests/web/playback-recovery.cjs
// The UI's build is read from web-rs/dist (or ENCORE_RS_DIST) and served at /, its hashed .js/.wasm at the root as on
// the server. A tiny local server serves it and a fake /api: songs are generated tones, sent as server-made streams
// (Data saver: a lossy stream sent chunked, paced, its clock starting at ?t=). The first stream of song 1 is cut
// mid-song – cleanly ('end': the browser takes it for the whole song) or abruptly ('drop': a network error). The
// player must not skip: it loads song 1 again near the cut and plays on; when song 1 really ends, song 2 plays.
// 'error': the connection drops and the browser's own attempt to fetch the stream again is refused (a network error on
// the element). 'short': nothing is cut, but the library says song 1 is longer than its file (30 s for 16): it really
// ends at 16 s, the player takes that for a break-off and asks past the file's end, which gives an empty stream (an
// error on the element, as AAC does) – song 2 must follow, with no "Couldn't keep … playing" and no long silence.
const http = require('http');
const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFileSync } = require('child_process');
const { chromium } = require('playwright');

const RS = path.resolve(process.env.ENCORE_RS_DIST || path.join(__dirname, '../../web-rs/dist'));   // the Rust/WASM UI's build, served at /
if (!fs.existsSync(path.join(RS, 'index.html'))) {
  console.error(`no web-rs build at ${RS}: run "cd web-rs && trunk build --release" or set ENCORE_RS_DIST`);
  process.exit(2);
}
const TMP = fs.mkdtempSync(path.join(os.tmpdir(), 'encore-pb-'));
const TRACKS = [
  { id: 1, title: 'Tone A', artist: 'Test', album: 'Tones', album_id: null, duration: 16, format: 'flac' },
  { id: 2, title: 'Tone B', artist: 'Test', album: 'Tones', album_id: null, duration: 6, format: 'flac' },
];
for (const t of TRACKS) {
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', `sine=frequency=${330 + t.id * 110}:duration=${t.duration}:sample_rate=48000`,
    '-ac', '2', path.join(TMP, `${t.id}.wav`)]);
}
const MIME = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.svg': 'image/svg+xml', '.json': 'application/json', '.wasm': 'application/wasm', '.webmanifest': 'application/manifest+json' };
// what the library says about the songs: in 'short', song 1 claims 30 s (its file has 16)
const listed = (mode) => TRACKS.map(t => mode === 'short' && t.id === 1 ? { ...t, duration: 30 } : t);
const sleep = (ms) => new Promise(r => setTimeout(r, ms));

let scenario = 'none';
let log = [];

function stream(req, res, id, q) {
  const t = Number(q.get('t')) || 0;
  const codec = q.get('q') === 'aac' ? ['-c:a', 'aac', '-b:a', '96k', '-f', 'adts'] : ['-c:a', 'libopus', '-b:a', '96k', '-f', 'ogg'];
  const data = execFileSync('ffmpeg', ['-v', 'error', '-ss', String(t), '-i', path.join(TMP, `${id}.wav`), ...codec, 'pipe:1'], { maxBuffer: 1 << 26 });
  const first = !log.some(l => l.kind === 'stream' && l.id === id);
  log.push({ kind: 'stream', id, t, at: Date.now(), q: q.toString() });
  const track0 = TRACKS.find(x => x.id === id);
  if (scenario === 'short' && t >= track0.duration - 0.5) {   // asked past the file's end: the server sends nothing
    res.writeHead(200, { 'Content-Type': q.get('q') === 'aac' ? 'audio/aac' : 'audio/ogg', 'Cache-Control': 'no-store' });
    res.end(); return;
  }
  if (scenario === 'error' && id === 1 && !first && t === 0) { res.writeHead(503); res.end(); return; }   // the browser asking again by itself
  const track = TRACKS.find(x => x.id === id);
  const secs = Math.max(0.5, track.duration - t);
  const perTick = Math.ceil(data.length / secs * 2 * 0.2);   // twice real time, every 200 ms
  const cut = first && id === 1 && scenario !== 'none' && scenario !== 'short' ? perTick * 15 : Infinity;   // ≈ 6 s of sound
  res.writeHead(200, { 'Content-Type': q.get('q') === 'aac' ? 'audio/aac' : 'audio/ogg', 'Cache-Control': 'no-store' });   // no length: chunked
  let sent = 0;
  const tick = () => {
    if (res.destroyed) return;
    if (sent >= cut) {
      log.push({ kind: 'cut', id, at: Date.now(), how: scenario });
      if (scenario === 'end') res.end(); else req.socket.destroy();
      return;
    }
    if (sent >= data.length) { res.end(); return; }
    const n = Math.min(perTick, data.length - sent, cut - sent);
    res.write(data.subarray(sent, sent + n));
    sent += n;
    setTimeout(tick, 200);
  };
  tick();
}

const server = http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  const json = (v) => { res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(v)); };
  const m = u.pathname.match(/^\/api\/library\/tracks\/(\d+)\/stream$/);
  if (m) return stream(req, res, Number(m[1]), u.searchParams);
  if (u.pathname.startsWith('/api/')) {
    let body = '';
    req.on('data', c => { body += c; });
    req.on('end', () => {
      if (u.pathname === '/api/library/plays' || u.pathname === '/api/library/now-playing') log.push({ kind: u.pathname.split('/').pop(), body: JSON.parse(body || '{}'), at: Date.now() });
      if (u.pathname === '/api/playback/dsp') return json({ enabled: false, allowed: false });
      if (u.pathname === '/api/auth/me') return json({ id: 1, username: 'test', role: 'owner', prefs: {} });
      if (u.pathname === '/api/auth/info') return json({ features: [] });
      if (u.pathname === '/api/library/shuffle' || u.pathname === '/api/likes') return json([]);
      json({});
    });
    return;
  }
  // the UI's files, index.html for the app's own routes (the server does the same)
  const rel = u.pathname;
  const f = rel === '/' || rel === '' ? path.join(RS, 'index.html') : path.join(RS, path.normalize(rel).replace(/^(\.\.[/\\])+/, ''));
  const file = f.startsWith(RS) && fs.existsSync(f) && !fs.statSync(f).isDirectory() ? f : (path.extname(rel) ? null : path.join(RS, 'index.html'));
  if (!file) { res.writeHead(404); return res.end(); }
  res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
  fs.createReadStream(file).pipe(res);
});

const failures = [];
const check = (ok, msg) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${msg}`); if (!ok) failures.push(msg); };

// what the page is playing: { id, pos }, read from the player bar
const sample = () => {
  const title = document.querySelector('#player .now .grow .ellipsis')?.textContent || '';
  const [m, sec] = (document.getElementById('t-cur')?.textContent || '0:00').split(':').map(Number);
  return { id: title === 'Tone A' ? 1 : title === 'Tone B' ? 2 : null, pos: m * 60 + sec };
};

async function run(browser, base, mode) {
  scenario = mode; log = [];
  const page = await browser.newPage();
  page.on('pageerror', e => console.log('page error:', e.message));
  const tag = `rust ${mode}`;
  await page.addInitScript((tracks) => {
    // what the playing element said (media events don't bubble: caught on the way down)
    window.__ev = [];
    for (const k of ['ended', 'error']) window.addEventListener(k, (e) => { if (e.target.id === 'audio' || e.target.id === 'audio-next') window.__ev.push(`${k}${e.target.error ? ' ' + e.target.error.code : ''}`); }, true);
    localStorage.setItem('encore.playback', JSON.stringify({ quality: 'saver', preload: false, autoplay: false, resume: true }));
    // the UI is started through its own pick-up: the queue put back paused, then play
    localStorage.setItem('encore.resume', JSON.stringify({ queue: tracks, index: 0, position: 0, context: 'album', repeat: 'off', saved_at: Date.now() / 1000 }));
  }, listed(mode));
  await page.goto(`${base}/`);
  await page.waitForFunction(() => document.querySelector('#p-toggle:not([disabled])') && window.encorePlayer, null, { timeout: 30000 });
  await page.evaluate(() => window.encorePlayer.toggle());
  const t0 = Date.now();
  const samples = [];
  let resumedAt = null;
  // watch for up to 40 s: song 1 (16 s) with its cut and pick-up, then song 2 (6 s)
  while (Date.now() - t0 < 40000) {
    const s = await page.evaluate(sample);
    samples.push({ ...s, at: Date.now() - t0 });
    const streams1 = log.filter(l => l.kind === 'stream' && l.id === 1);
    // (the browser may first ask for the same stream again by itself; ours asks for it at the spot it reached)
    if (resumedAt === null) resumedAt = streams1.find(l => l.t > 0)?.t ?? null;
    if (s.id === 2 && s.pos >= 5) break;
    await sleep(250);
  }
  await sleep(1500);   // song 2's own end, and its play count
  const events = await page.evaluate(() => window.__ev);
  const toasts = await page.evaluate(() => [...document.querySelectorAll('#toasts .toast')].map(t => t.textContent));
  const cut = log.find(l => l.kind === 'cut');
  const firstSong2 = samples.find(x => x.id === 2);
  const lastSong1 = [...samples].reverse().find(x => x.id === 1);
  const maxPos1 = Math.max(...samples.filter(x => x.id === 1).map(x => x.pos));
  console.log(`[${tag}] streams: ${log.filter(l => l.kind === 'stream').map(l => `#${l.id}@${l.t}`).join(', ')} · song 1 reached ${maxPos1.toFixed(1)} s · events: ${events.join(', ')} · toasts: ${JSON.stringify(toasts)}`);
  if (mode === 'short') {
    // song 1's real end: song 2 follows within a few seconds (no 1/2/4/8/16 s retries)
    const end1 = samples.filter(x => x.id === 1 && x.pos >= 14).pop();
    check(!!firstSong2 && !!end1 && firstSong2.at - end1.at < 6000, `[${tag}] song 2 followed song 1's real end at once (${firstSong2 && end1 ? ((firstSong2.at - end1.at) / 1000).toFixed(1) : '?'} s)`);
  } else {
    check(!!cut, `[${tag}] the server cut song 1's first stream`);
    check(resumedAt !== null && resumedAt >= 2 && resumedAt <= 10, `[${tag}] song 1 was loaded again near the cut (t=${resumedAt})`);
  }
  if (mode === 'error') check(events.some(e => e.startsWith('error')), `[${tag}] the element reported an error`);
  check(maxPos1 >= 13, `[${tag}] song 1 played on to its real end (reached ${maxPos1.toFixed(1)} s of 16)`);
  check(!!firstSong2 && lastSong1 && lastSong1.pos >= 13, `[${tag}] song 2 started only after song 1 really ended (song 1 last seen at ${lastSong1?.pos?.toFixed(1)} s)`);
  check(samples.some(x => x.id === 2 && x.pos > 4), `[${tag}] song 2 played`);
  check(!toasts.some(t => /Couldn't keep/.test(t)), `[${tag}] nothing was skipped`);
  check(log.filter(l => l.kind === 'plays' && l.body.track_id === 1).length === 1, `[${tag}] song 1's play was counted once`);
  check(log.filter(l => l.kind === 'now-playing' && l.body.track_id === 1).length === 1, `[${tag}] song 1 was announced once`);
  await page.close();
}

(async () => {
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  try {
    for (const mode of (process.env.MODES || 'end,drop,error,short').split(',')) await run(browser, base, mode);
  } finally {
    await browser.close();
    server.close();
    fs.rmSync(TMP, { recursive: true, force: true });
  }
  console.log(failures.length ? `${failures.length} failed` : 'all passed');
  process.exit(failures.length ? 1 : 0);
})();
