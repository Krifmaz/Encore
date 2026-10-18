/* global api, settings, handoffBlocker */
// The extension's one page: the toolbar popup (popup.html), and the options page from the browser's extension
// settings (options.html forwards to popup.html?tab, which gets the wider page layout). Everything is here: the
// hand-off, the everyday-browser sync, and the Encore address and token.
const $ = (id) => document.getElementById(id);
const inTab = new URLSearchParams(location.search).has('tab');
if (inTab) document.documentElement.classList.add('tab');

const COOKIE_ORIGINS = ['*://*.youtube.com/*', '*://*.google.com/*'];
// a click that ended in the permission prompt is remembered this long (the popup may close on the prompt)
const PENDING_MS = 10 * 60 * 1000;
const AGAIN = { handoff: 'Click Hand over a YouTube sign-in once more to start.', send: 'Click Send now once more.' };
// load() has put the stored values into the fields and the switch
let shown = false;

// "music.example.com/" → "https://music.example.com" (an address with another scheme is left for originsFor to refuse)
function normalise(raw) {
  let url = raw.trim().replace(/\/+$/, '');
  if (url && !/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) url = `https://${url}`;
  return url;
}

// The permissions an Encore address needs: reaching it, and the YouTube cookies. The pattern has no port:
// Firefox's match patterns can't hold one, and without one Chrome's match every port. Throws on anything
// that is not an http(s) address with a plain host name.
function originsFor(url) {
  const u = new URL(url);
  if (!/^https?:$/.test(u.protocol) || !u.hostname || /[\s%]/.test(u.hostname)) throw new TypeError('not a web address');
  return [`${u.protocol}//${u.hostname}/*`, ...COOKIE_ORIGINS];
}

function hostOf(url) {
  try { return new URL(url).host; } catch { return url; }
}

function when(at) {
  const d = new Date(at);
  if (d.toDateString() === new Date().toDateString()) return `today ${d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`;
  return d.toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
}

// A status line: an optional badge (cls ok, warn or err), then the text and the time.
function line(el, { cls = '', badge = '', text = '', at = 0 } = {}) {
  el.replaceChildren();
  if (badge) {
    const b = document.createElement('span');
    b.className = `badge ${cls}`;
    b.textContent = badge;
    el.append(b);
  }
  const rest = [text, at ? when(at) : ''].filter(Boolean).join(' · ');
  if (rest) {
    const t = document.createElement('span');
    t.textContent = rest;
    el.append(t);
  }
  el.hidden = !badge && !rest;
}

// The last upload or hand-off, as remember() in shared.js keeps it: {at, ok, message}.
function result(el, last, okBadge, empty) {
  if (!last) { line(el, { text: empty }); return; }
  const text = last.ok && last.message === okBadge ? '' : last.message;
  line(el, { cls: last.ok ? 'ok' : 'err', badge: last.ok ? okBadge : 'Failed', text, at: last.at });
}

function connHost(url) {
  $('connHost').textContent = url ? hostOf(url) : 'Not set up';
}

// Shows what is stored: the address and token, the switch, the last upload and hand-off, and whether the
// permission for the saved address is there. A click that ended in a permission prompt (see saveAndAsk) is
// picked up here: Firefox closes the popup when its prompt opens, so the popup only learns the answer when it
// opens again.
async function load() {
  const s = await api.storage.local.get(['url', 'token', 'last', 'lastHandoff', 'pending']);
  const cfg = await settings();
  $('url').value = s.url || '';
  $('token').value = s.token || '';
  $('everyday').checked = cfg.everyday;
  shown = true;
  connHost(cfg.url);
  const ready = !!(cfg.url && cfg.token);
  document.body.classList.toggle('setup', !ready);
  $('conn').open = inTab || !ready;
  result($('status'), s.last, 'Sent', 'Not sent yet.');
  result($('handoffStatus'), s.lastHandoff, 'Handed over', '');
  const pending = s.pending && Date.now() - s.pending.at < PENDING_MS ? s.pending : null;
  if (s.pending) await api.storage.local.remove('pending');
  if (!ready) {
    if (pending && cfg.url) line($('saved'), { cls: 'warn', badge: 'Saved', text: 'Now paste the token from Encore.' });
    $(cfg.url ? 'token' : 'url').focus();
    return;
  }

  let allowed = false;
  try { allowed = await api.permissions.contains({ origins: originsFor(cfg.url) }); } catch { allowed = false; }
  if (!allowed) {
    // the connection comes first again, so this is seen without scrolling
    document.body.classList.add('setup');
    $('conn').open = true;
    line($('saved'), { cls: 'warn', badge: 'Not allowed yet', text: `The extension needs permission to reach ${hostOf(cfg.url)} and read the YouTube cookies. Click Save once more and allow it.` });
    return;
  }
  if (pending && AGAIN[pending.action]) {
    line(pending.action === 'send' ? $('status') : $('handoffStatus'), { cls: 'ok', badge: 'Allowed', text: AGAIN[pending.action] });
  } else if (pending) {
    line($('saved'), { cls: 'ok', badge: 'Saved' });
  }
  const blocker = await handoffBlocker();
  if (blocker && pending?.action !== 'handoff') line($('handoffStatus'), { cls: 'warn', badge: 'One more step', text: blocker });
}

// Saves the address and token, then asks for permission to reach that Encore and read the YouTube cookies
// (answers at once, without a prompt, when it is already given). Called straight from a click, and nothing is
// awaited before permissions.request: Firefox asks only from a click. Firefox also closes the popup when its
// prompt opens (Chrome may too), so whatever runs after the prompt is lost there: the address and token are
// written first, with the click as `pending`, and load() checks the permission when the popup opens again.
// Resolves to true when saved and allowed.
function saveAndAsk(action) {
  const url = normalise($('url').value);
  const token = $('token').value.trim();
  let origins;
  try { origins = originsFor(url); } catch {
    $('conn').open = true;
    line($('saved'), { cls: 'err', badge: url ? 'Not a web address' : 'Missing', text: url ? 'Enter the address you open Encore at, like https://music.example.com.' : 'Enter the Encore address first.' });
    $('url').focus();
    return Promise.resolve(false);
  }
  $('url').value = url;
  $('token').value = token;
  line($('saved')); // an older message would stay up while the browser's prompt is open
  // The switch as shown is written too: settings() treats a stored address and token without `everyday` as an
  // install from before the switch, which keeps syncing, so a new install would start syncing unasked.
  const write = api.storage.local.set({ url, token, pending: { action, at: Date.now() }, ...(shown ? { everyday: $('everyday').checked } : {}) });
  let asked;
  try { asked = api.permissions.request({ origins }); } catch (e) { asked = Promise.reject(e); }
  return Promise.all([write, asked]).then(async ([, granted]) => {
    await api.storage.local.remove('pending');
    connHost(url);
    if (!granted) {
      $('conn').open = true;
      line($('saved'), { cls: 'err', badge: 'Not allowed', text: `The extension needs permission to reach ${hostOf(url)} and read the YouTube cookies.` });
      $('saved').scrollIntoView({ block: 'nearest' });
      return false;
    }
    if (!token) {
      $('conn').open = true;
      line($('saved'), { cls: 'warn', badge: 'Saved', text: 'Now paste the token from Encore.' });
      $('token').focus();
      return false;
    }
    line($('saved'), { cls: 'ok', badge: 'Saved' });
    if (document.body.classList.contains('setup')) {
      document.body.classList.remove('setup');
      // set up for the first time: the connection folds away and the hand-off is next
      if (action === 'save' && !inTab) {
        $('conn').open = false;
        line($('handoffStatus'), { cls: 'ok', badge: 'Saved', text: `Encore at ${hostOf(url)} is set up. Hand over a YouTube sign-in next.` });
      }
    }
    return true;
  }, async (e) => {
    await api.storage.local.remove('pending');
    line($('saved'), { cls: 'err', badge: 'Failed', text: `Could not ask for the permission: ${e.message}` });
    return false;
  });
}

async function sendNow() {
  line($('status'), { text: 'Sending…' });
  await api.runtime.sendMessage({ type: 'sync-now' }).catch(() => null);
  result($('status'), (await api.storage.local.get('last')).last, 'Sent', 'Not sent yet.');
}

// a click (Enter in a field clicks it too), not the form's submit event: the permission prompt needs the click
$('save').addEventListener('click', (e) => {
  e.preventDefault();
  saveAndAsk('save');
});
$('connForm').addEventListener('submit', (e) => e.preventDefault());
// opened at the bottom of a set-up popup, the form would sit below the fold
$('conn').addEventListener('toggle', () => { if ($('conn').open && !inTab) $('conn').scrollIntoView({ block: 'nearest' }); });
$('showToken').addEventListener('click', () => {
  const show = $('token').type === 'password';
  $('token').type = show ? 'text' : 'password';
  $('showToken').textContent = show ? 'Hide' : 'Show';
  $('showToken').setAttribute('aria-pressed', String(show));
});
$('everyday').addEventListener('change', async () => {
  await api.storage.local.set({ everyday: $('everyday').checked });
  if ($('everyday').checked) await sendNow();
});
$('send').addEventListener('click', () => {
  saveAndAsk('send').then((ok) => ok && sendNow());
});
$('handoff').addEventListener('click', () => {
  const hs = $('handoffStatus');
  saveAndAsk('handoff').then(async (ok) => {
    if (!ok) return;
    const blocker = await handoffBlocker();
    if (blocker) { line(hs, { cls: 'warn', badge: 'One more step', text: blocker }); return; }
    line(hs, { text: 'Opening a private window…' });
    // the background opens it: a popup closes when the new window takes the focus
    const r = await api.runtime.sendMessage({ type: 'handoff-start' }).catch((e) => ({ ok: false, message: e.message }));
    if (r && r.ok) line(hs, { text: 'Sign in to Google in the private window.' });
    else line(hs, { cls: 'err', badge: 'Failed', text: r?.message || 'Could not open a private window' });
  });
});
api.storage.onChanged.addListener((ch, area) => {
  if (area && area !== 'local') return;
  if (ch.last) result($('status'), ch.last.newValue, 'Sent', 'Not sent yet.');
  if (ch.lastHandoff) result($('handoffStatus'), ch.lastHandoff.newValue, 'Handed over', '');
});
load();
