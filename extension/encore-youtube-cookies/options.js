const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);

function show(last) {
  const s = $('status');
  if (!last) { s.textContent = 'Not sent yet.'; s.className = ''; return; }
  s.textContent = `${last.message} · ${new Date(last.at).toLocaleString()}`;
  s.className = last.ok ? 'ok' : 'err';
}

async function load() {
  const s = await api.storage.local.get(['url', 'token', 'last']);
  $('url').value = s.url || '';
  $('token').value = s.token || '';
  show(s.last);
}

async function sendNow() {
  $('status').textContent = 'Sending…';
  $('status').className = '';
  await api.runtime.sendMessage({ type: 'sync-now' });
  show((await api.storage.local.get('last')).last);
}

$('save').addEventListener('click', async () => {
  let url = $('url').value.trim().replace(/\/+$/, '');
  if (url && !/^https?:\/\//.test(url)) url = `https://${url}`;
  let origin;
  try { origin = new URL(url).origin; } catch { $('status').textContent = 'That is not a web address'; $('status').className = 'err'; return; }
  // the Encore server, plus YouTube/Google (Firefox asks for these on first use)
  const granted = await api.permissions.request({ origins: [`${origin}/*`, '*://*.youtube.com/*', '*://*.google.com/*'] });
  if (!granted) { $('status').textContent = 'The extension needs permission to reach Encore and read the YouTube cookies'; $('status').className = 'err'; return; }
  await api.storage.local.set({ url, token: $('token').value.trim() });
  $('url').value = url;
  await sendNow();
});
$('send').addEventListener('click', sendNow);
load();
