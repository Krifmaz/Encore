/* global api, settings, handoffBlocker */
const $ = (id) => document.getElementById(id);

function show(el, last, empty) {
  if (!last) { el.textContent = empty; el.className = 'status'; return; }
  el.textContent = `${last.message} · ${new Date(last.at).toLocaleString()}`;
  el.className = `status ${last.ok ? 'ok' : 'err'}`;
}

function say(el, text, cls) { el.textContent = text; el.className = `status ${cls || ''}`; }

async function load() {
  const s = await api.storage.local.get(['url', 'token', 'last', 'lastHandoff']);
  const cfg = await settings();
  $('url').value = s.url || '';
  $('token').value = s.token || '';
  $('everyday').checked = cfg.everyday;
  show($('status'), s.last, 'Not sent yet.');
  show($('handoffStatus'), s.lastHandoff, '');
}

// Save the address and token (asks for permission to reach that Encore and read the YouTube cookies).
async function save() {
  let url = $('url').value.trim().replace(/\/+$/, '');
  if (url && !/^https?:\/\//.test(url)) url = `https://${url}`;
  let origin;
  try { origin = new URL(url).origin; } catch { say($('saved'), 'That is not a web address', 'err'); return false; }
  const granted = await api.permissions.request({ origins: [`${origin}/*`, '*://*.youtube.com/*', '*://*.google.com/*'] });
  if (!granted) { say($('saved'), 'The extension needs permission to reach Encore and read the YouTube cookies', 'err'); return false; }
  await api.storage.local.set({ url, token: $('token').value.trim(), everyday: $('everyday').checked });
  $('url').value = url;
  say($('saved'), 'Saved.', 'ok');
  return true;
}

async function sendNow() {
  say($('status'), 'Sending…');
  await api.runtime.sendMessage({ type: 'sync-now' });
  show($('status'), (await api.storage.local.get('last')).last, 'Not sent yet.');
}

$('save').addEventListener('click', save);
$('everyday').addEventListener('change', async () => {
  await api.storage.local.set({ everyday: $('everyday').checked });
  if ($('everyday').checked) await sendNow();
});
$('send').addEventListener('click', async () => { if (await save()) await sendNow(); });
$('handoff').addEventListener('click', async () => {
  // keep what was typed: the hand-off needs it (save() first: Firefox asks for permissions only straight from a click)
  if ($('url').value.trim() && !(await save())) return;
  const blocker = await handoffBlocker();
  if (blocker) { say($('handoffStatus'), blocker, 'err'); return; }
  say($('handoffStatus'), 'Opening a private window…');
  const r = await api.runtime.sendMessage({ type: 'handoff-start' }).catch((e) => ({ ok: false, message: e.message }));
  say($('handoffStatus'), r && r.ok ? 'Sign in to Google in the private window.' : (r?.message || 'Could not open a private window'), r && r.ok ? '' : 'err');
});
api.storage.onChanged.addListener((ch) => { if (ch.lastHandoff) show($('handoffStatus'), ch.lastHandoff.newValue, ''); });
load();
