/* global api, handoffBlocker */
const $ = (id) => document.getElementById(id);

function say(text, cls) { $('status').textContent = text; $('status').className = `status ${cls || ''}`; }

async function load() {
  const s = await api.storage.local.get(['lastHandoff', 'last']);
  const blocker = await handoffBlocker();
  if (blocker) { say(blocker, 'err'); return; }
  const l = s.lastHandoff;
  if (l) say(`Last hand-off: ${l.message} · ${new Date(l.at).toLocaleString()}`, l.ok ? 'ok' : 'err');
}

$('handoff').addEventListener('click', async () => {
  const blocker = await handoffBlocker();
  if (blocker) { say(blocker, 'err'); return; }
  say('Opening a private window…');
  // the background opens it: this popup closes when the new window takes the focus
  const r = await api.runtime.sendMessage({ type: 'handoff-start' }).catch(() => null);
  if (r && !r.ok) say(r.message, 'err');
});
$('options').addEventListener('click', () => api.runtime.openOptionsPage());
load();
