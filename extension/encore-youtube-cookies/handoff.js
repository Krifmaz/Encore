/* global api, collect, signedIn, post, remember */
// Watches the private window: once music.youtube.com is loaded and signed in (SAPISID / __Secure-3PAPISID in
// that window's cookie store), sends the store's youtube.com / google.com cookies to Encore with handoff:true
// after a visible 3 s countdown (or at once with the button), then closes the window WITHOUT signing out.
const $ = (id) => document.getElementById(id);
const COUNT_FROM = 3;
let winId = null;
let selfTab = null;
let phase = 'waiting'; // waiting → counting → sending → done | held (cancelled countdown) | failed
let storeId = null;
let timer = null;

function say(text, cls) { $('state').textContent = text; $('state').className = `status ${cls || ''}`; }

// The cookie store of a tab: Firefox names it on the tab, Chrome lists the tabs of each store.
async function storeOf(tab) {
  if (tab.cookieStoreId) return tab.cookieStoreId;
  const stores = await api.cookies.getAllCookieStores();
  return (stores.find(s => (s.tabIds || []).includes(tab.id)) || {}).id;
}

async function look() {
  if (phase !== 'waiting') return;
  let tabs;
  try { tabs = await api.tabs.query({ windowId: winId }); } catch { tabs = null; }
  if (!tabs) { phase = 'failed'; say('The private window was closed before the sign-in was sent. Start again from the extension button.', 'err'); return; }
  const yt = tabs.find(t => (t.url || '').startsWith('https://music.youtube.com') && t.status === 'complete');
  if (!yt) { say('Waiting for the sign-in… (sign in to Google in the other tab; it continues to YouTube Music)'); return; }
  const id = await storeOf(yt);
  const cookies = await collect(id);
  if (!signedIn(cookies)) { say('YouTube Music is open but not signed in yet – use its Sign in button.'); return; }
  storeId = id;
  countdown();
}

function countdown() {
  phase = 'counting';
  // bring this page to the front so the countdown is seen
  if (selfTab && selfTab.windowId === winId) api.tabs.update(selfTab.id, { active: true }).catch(() => {});
  say('Signed in. Sending to Encore in…', 'ok');
  $('send').hidden = false;
  $('cancel').hidden = false;
  let n = COUNT_FROM;
  $('count').hidden = false;
  $('count').textContent = String(n);
  timer = setInterval(() => {
    n -= 1;
    $('count').textContent = String(Math.max(n, 0));
    if (n <= 0) send();
  }, 1000);
}

function stopTimer() {
  if (timer) clearInterval(timer);
  timer = null;
  $('count').hidden = true;
}

async function send() {
  if (phase === 'sending' || phase === 'done') return;
  stopTimer();
  phase = 'sending';
  $('send').disabled = true;
  $('cancel').hidden = true;
  say('Sending to Encore…');
  const res = await post(await collect(storeId), { handoff: true });
  await remember(res.ok, res.ok ? 'Handed over' : res.message, 'handoff');
  if (!res.ok) {
    phase = 'failed';
    $('send').disabled = false;
    $('send').textContent = 'Send again';
    say(res.message, 'err');
    return;
  }
  phase = 'done';
  $('send').hidden = true;
  // the account name is looked up after the answer: the YouTube card in Settings shows it in a moment
  say('Done – Encore has its own YouTube sign-in now and keeps it alive by itself. Closing the private window…', 'ok');
  // closing (not signing out): the private window's cookies are thrown away, Encore's session stays valid
  setTimeout(() => { api.windows.remove(winId).catch(() => {}); }, 1500);
}

$('send').addEventListener('click', () => { if (phase === 'failed') phase = 'counting'; send(); });
$('cancel').addEventListener('click', () => {
  stopTimer();
  phase = 'held';
  $('cancel').hidden = true;
  say('Not sent. Press Send to Encore when you are ready.');
});

(async () => {
  selfTab = await api.tabs.getCurrent().catch(() => null);
  const w = new URLSearchParams(location.search).get('win');
  winId = w ? Number(w) : selfTab?.windowId;
  if (winId == null) { say('Open this from the extension button (Hand over a YouTube sign-in).', 'err'); return; }
  api.tabs.onUpdated.addListener((_id, info, tab) => { if (tab.windowId === winId && info.status === 'complete') look(); });
  setInterval(look, 2000);
  look();
})();
