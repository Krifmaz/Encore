// Encore – YouTube sign-in: sends this browser's youtube.com / google.com cookies to an Encore server
// (POST <Encore>/api/youtube/extension-cookies with the token from Settings → YouTube sign-in).
// When: on install and browser start, 30 s after a sign-in cookie changes, and every 6 hours.
// Encore never rotates these cookies itself, so the browser stays signed in.
const api = globalThis.browser ?? globalThis.chrome;

const DOMAINS = ['youtube.com', 'google.com'];
// cookies whose change means a new sign-in state (the many tracking cookies are left alone)
const SIGN_IN = new Set(['SID', 'HSID', 'SSID', 'APISID', 'SAPISID', 'LOGIN_INFO', '__Secure-1PSID', '__Secure-3PSID',
  '__Secure-1PAPISID', '__Secure-3PAPISID', '__Secure-1PSIDTS', '__Secure-3PSIDTS']);
const EVERY_MINUTES = 360;
const DEBOUNCE_MS = 30 * 1000;

function agent() {
  const ua = navigator.userAgent;
  const m = ua.match(/(Firefox|Edg|OPR|Chrome)\/(\d+)/);
  if (!m) return 'Browser';
  const name = { Edg: 'Edge', OPR: 'Opera' }[m[1]] || m[1];
  return `${name} ${m[2]}`;
}

async function settings() {
  const s = await api.storage.local.get(['url', 'token']);
  return { url: (s.url || '').trim().replace(/\/+$/, ''), token: (s.token || '').trim() };
}

async function remember(ok, message) {
  await api.storage.local.set({ last: { at: Date.now(), ok, message } });
}

async function sync(reason) {
  const { url, token } = await settings();
  if (!url || !token) { await remember(false, 'Enter the Encore address and token in the options'); return { ok: false }; }
  const lists = await Promise.all(DOMAINS.map(domain => api.cookies.getAll({ domain })));
  const seen = new Set();
  const cookies = [];
  for (const c of lists.flat()) {
    const key = `${c.domain}|${c.path}|${c.name}`;
    if (seen.has(key)) continue;
    seen.add(key);
    cookies.push({ domain: c.domain, hostOnly: c.hostOnly, name: c.name, value: c.value, path: c.path, secure: c.secure,
      httpOnly: c.httpOnly, session: c.session, expirationDate: c.expirationDate });
  }
  try {
    const r = await fetch(`${url}/api/youtube/extension-cookies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
      body: JSON.stringify({ cookies, agent: agent(), reason }),
    });
    const body = await r.json().catch(() => ({}));
    if (!r.ok) { await remember(false, body.detail || `Encore answered HTTP ${r.status}`); return { ok: false }; }
    await remember(!!body.ok, body.message || (body.ok ? 'Sent' : 'Not signed in to YouTube'));
    return body;
  } catch (e) {
    await remember(false, `Could not reach Encore: ${e.message}`);
    return { ok: false };
  }
}

function schedule() {
  api.alarms.create('encore-every', { periodInMinutes: EVERY_MINUTES });
}

api.runtime.onInstalled.addListener(() => { schedule(); sync('install'); });
api.runtime.onStartup.addListener(() => { schedule(); sync('start'); });
api.alarms.onAlarm.addListener((a) => {
  if (a.name === 'encore-every') sync('timer');
  else if (a.name === 'encore-changed') sync('changed');
});
api.cookies.onChanged.addListener(({ cookie }) => {
  const d = (cookie.domain || '').replace(/^\./, '');
  if (!DOMAINS.some(x => d === x || d.endsWith(`.${x}`)) || !SIGN_IN.has(cookie.name)) return;
  // debounced: one upload 30 s after the last change (alarms survive the service worker going to sleep)
  api.alarms.create('encore-changed', { when: Date.now() + DEBOUNCE_MS });
});
api.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg && msg.type === 'sync-now') { sync('options').then(reply); return true; }
  return false;
});
api.action?.onClicked?.addListener(() => api.runtime.openOptionsPage());
