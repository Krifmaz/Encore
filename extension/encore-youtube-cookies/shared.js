// Shared by the background script, the popup, the options page and the hand-off page.
// Nothing here logs or shows a cookie value.
/* exported api, SIGN_IN_URL, DOMAINS, settings, agent, remember, collect, post, signedIn, incognitoAllowed, isFirefox */
const api = globalThis.browser ?? globalThis.chrome;
// Google's sign-in page, continuing to YouTube Music
const SIGN_IN_URL = 'https://accounts.google.com/ServiceLogin?continue=https://music.youtube.com/';
const DOMAINS = ['youtube.com', 'google.com'];
const isFirefox = typeof globalThis.browser !== 'undefined' && /Firefox\//.test(navigator.userAgent);

function agent() {
  const ua = navigator.userAgent;
  const m = ua.match(/(Firefox|Edg|OPR|Chrome)\/(\d+)/);
  if (!m) return 'Browser';
  const name = { Edg: 'Edge', OPR: 'Opera' }[m[1]] || m[1];
  return `${name} ${m[2]}`;
}

async function settings() {
  const s = await api.storage.local.get(['url', 'token', 'everyday']);
  const url = (s.url || '').trim().replace(/\/+$/, '');
  const token = (s.token || '').trim();
  // installs from before the hand-off synced the everyday browser: they keep doing so until switched off
  const everyday = typeof s.everyday === 'boolean' ? s.everyday : !!(url && token);
  return { url, token, everyday };
}

async function remember(ok, message, kind) {
  await api.storage.local.set({ [kind === 'handoff' ? 'lastHandoff' : 'last']: { at: Date.now(), ok, message } });
}

// The youtube.com / google.com cookies of one cookie store (undefined: this context's own store).
async function collect(storeId) {
  const q = (domain) => api.cookies.getAll(storeId ? { domain, storeId } : { domain });
  const lists = await Promise.all(DOMAINS.map(q));
  const seen = new Set();
  const cookies = [];
  for (const c of lists.flat()) {
    const key = `${c.domain}|${c.path}|${c.name}`;
    if (seen.has(key)) continue;
    seen.add(key);
    cookies.push({ domain: c.domain, hostOnly: c.hostOnly, name: c.name, value: c.value, path: c.path, secure: c.secure,
      httpOnly: c.httpOnly, session: c.session, expirationDate: c.expirationDate });
  }
  return cookies;
}

// Signed in to YouTube: the SAPISID cookies are there.
function signedIn(cookies) {
  return cookies.some(c => /(^|\.)youtube\.com$/.test(c.domain.replace(/^\./, '')) && (c.name === 'SAPISID' || c.name === '__Secure-3PAPISID'));
}

// POST the cookies to Encore. handoff: the private window's sign-in becomes Encore's own.
async function post(cookies, extra) {
  const { url, token } = await settings();
  if (!url || !token) return { ok: false, message: 'Enter the Encore address and token in the options first' };
  try {
    const r = await fetch(`${url}/api/youtube/extension-cookies`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
      body: JSON.stringify({ cookies, agent: agent(), ...extra }),
    });
    const body = await r.json().catch(() => ({}));
    if (!r.ok) return { ok: false, message: body.detail || body.error || `Encore answered HTTP ${r.status}` };
    return { ...body, ok: !!body.ok, message: body.message || (body.ok ? 'Sent' : 'Not signed in to YouTube') };
  } catch (e) {
    return { ok: false, message: `Could not reach Encore: ${e.message}` };
  }
}

// May the extension open (and read) a private window?
async function incognitoAllowed() {
  try { return !!(await api.extension.isAllowedIncognitoAccess()); } catch { return false; }
}

// Why the hand-off can't start yet (null: it can).
async function handoffBlocker() {
  const s = await settings();
  if (!s.url || !s.token) return 'Enter the Encore address and token in the options first.';
  if (!(await incognitoAllowed())) {
    return isFirefox
      ? 'Allow the extension in private windows first: open about:addons → Encore – YouTube sign-in → "Run in Private Windows" → Allow. Then press the button again.'
      : 'Allow the extension in incognito first: open chrome://extensions (edge://extensions in Edge, brave://extensions in Brave) → Encore – YouTube sign-in → Details → switch on "Allow in Incognito". Then press the button again.';
  }
  return null;
}

// Open a NEW private window at Google's sign-in, with the hand-off page (handoff.html) watching it.
// Runs in the background script (a popup closes as soon as the new window takes the focus).
async function openHandoffWindow() {
  const page = api.runtime.getURL('handoff.html');
  let win;
  try {
    // the hand-off page inside the private window itself (Chrome's split incognito mode allows this)
    win = await api.windows.create({ incognito: true, url: [SIGN_IN_URL, page], focused: true });
  } catch {
    win = null;
  }
  const tabs = win?.tabs || [];
  if (!win || !tabs.some(t => (t.url || t.pendingUrl || '').startsWith(page))) {
    // extension pages not allowed in private windows (Firefox): the page watches from a normal tab
    if (!win) win = await api.windows.create({ incognito: true, url: SIGN_IN_URL, focused: true });
    await api.tabs.create({ url: `${page}?win=${win.id}`, active: false });
  }
  const signIn = (win.tabs || []).find(t => !(t.url || t.pendingUrl || '').startsWith(page));
  if (signIn) { try { await api.tabs.update(signIn.id, { active: true }); } catch { /* fine */ } }
  return { ok: true };
}
