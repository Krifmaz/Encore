// Encore – YouTube sign-in, background part.
// * Hand-off (recommended, handoff.html): a new private window signs in for Encore only; its cookies are sent
//   with handoff:true and the window closes without signing out, so Encore owns that session alone and keeps
//   it alive by itself.
// * Everyday browser sync (optional, switched on in the options): this browser's own youtube.com / google.com
//   cookies, on browser start, 30 s after a sign-in cookie changes and every 6 hours. Encore never rotates
//   these, so the browser stays signed in.
if (typeof importScripts === 'function') importScripts('shared.js'); // Chrome service worker (Firefox lists it in the manifest)

// cookies whose change means a new sign-in state (the many tracking cookies are left alone)
const SIGN_IN = new Set(['SID', 'HSID', 'SSID', 'APISID', 'SAPISID', 'LOGIN_INFO', '__Secure-1PSID', '__Secure-3PSID',
  '__Secure-1PAPISID', '__Secure-3PAPISID', '__Secure-1PSIDTS', '__Secure-3PSIDTS']);
const EVERY_MINUTES = 360;
const DEBOUNCE_MS = 30 * 1000;
// Chrome's incognito store, Firefox's private one: never part of the everyday sync
const PRIVATE_STORES = new Set(['1', 'firefox-private']);
const incognitoContext = !!api.extension.inIncognitoContext;

async function sync(reason, force) {
  // the incognito copy of this script (Chrome split mode) never syncs: that is the hand-off's window
  if (incognitoContext) return { ok: false };
  const s = await settings();
  if (!force && !s.everyday) return { ok: false };
  if (!s.url || !s.token) { await remember(false, 'Enter the Encore address and token in the options'); return { ok: false }; }
  const res = await post(await collect(), { reason });
  await remember(res.ok, res.message);
  return res;
}

function schedule() {
  if (!incognitoContext) api.alarms.create('encore-every', { periodInMinutes: EVERY_MINUTES });
}

api.runtime.onInstalled.addListener(() => { schedule(); sync('install'); });
api.runtime.onStartup.addListener(() => { schedule(); sync('start'); });
api.alarms.onAlarm.addListener((a) => {
  if (a.name === 'encore-every') sync('timer');
  else if (a.name === 'encore-changed') sync('changed');
});
api.cookies.onChanged.addListener(({ cookie }) => {
  if (incognitoContext || PRIVATE_STORES.has(cookie.storeId)) return;
  const d = (cookie.domain || '').replace(/^\./, '');
  if (!DOMAINS.some(x => d === x || d.endsWith(`.${x}`)) || !SIGN_IN.has(cookie.name)) return;
  // debounced: one upload 30 s after the last change (alarms survive the service worker going to sleep)
  api.alarms.create('encore-changed', { when: Date.now() + DEBOUNCE_MS });
});
api.runtime.onMessage.addListener((msg, _sender, reply) => {
  if (msg && msg.type === 'sync-now') { sync('options', true).then(reply); return true; }
  if (msg && msg.type === 'handoff-start' && !incognitoContext) {
    handoffBlocker().then(b => b ? { ok: false, message: b } : openHandoffWindow())
      .catch(e => ({ ok: false, message: `Could not open a private window: ${e.message}` })).then(reply);
    return true;
  }
  return false;
});
