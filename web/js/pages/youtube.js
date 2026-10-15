// Settings → YouTube sign-in: which cookies yt-dlp uses (Encore's browser, the browser extension or pasted
// cookies), the live view for signing Encore's browser in, Refresh now / Sign out, and the extension token.
// Mirrored in web-rs/src/pages/youtube.rs (same words, same DOM).
import { api } from '../api.js';
import { copyText, esc, icon, toast } from '../ui.js';

const ago = (t) => { if (!t) return 'never'; const s = Date.now() / 1000 - t; return s < 90 ? 'just now' : s < 5400 ? `${Math.round(s / 60)} min ago` : s < 129600 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`; };
const until = (t) => { if (!t) return ''; const s = t - Date.now() / 1000; return s <= 60 ? 'any moment' : s < 5400 ? `in ${Math.round(s / 60)} min` : s < 129600 ? `in ${Math.round(s / 3600)} h` : `in ${Math.round(s / 86400)} days`; };

/** The card's frame; the status inside is filled in by bindYoutube. */
export function youtubeBlock() {
  return `<div class="card flat" id="yt-auth" style="padding:12px 14px;margin-bottom:14px">
    <div data-yt-status><div class="small muted">Loading…</div></div>
    <div class="row wrap mt" style="gap:8px">
      <button class="btn sm primary" type="button" data-yt="signin">${icon.user}Sign in to YouTube</button>
      <button class="btn sm" type="button" data-yt="refresh">${icon.refresh}Refresh now</button>
      <button class="btn sm" type="button" data-yt="extension">${icon.link}Connect browser extension</button>
      <button class="btn sm danger" type="button" data-yt="signout">${icon.x}Sign out</button>
    </div>
    <div data-yt-ext hidden></div>
    <div data-yt-live hidden></div>
  </div>`;
}

const SOURCE_WORDS = { managed: 'Encore’s browser', extension: 'Browser extension', pasted: 'Pasted cookies' };

function sourceLine(key, s) {
  let state;
  if (!s || !s.has_cookies) state = key === 'extension' ? 'not connected' : key === 'managed' ? 'not signed in' : 'none';
  else if (s.signed_in) state = `signed in${s.account ? ` as ${s.account}` : ''}`;
  else state = 'signed out';
  return `<span>${esc(SOURCE_WORDS[key])}: ${esc(state)}</span>`;
}

/** The status part of the card (from GET /api/youtube/auth). */
export function youtubeStatusHtml(r) {
  const src = r.source ? r.sources[r.source] : null;
  const ok = !!(src && src.signed_in);
  const badge = !r.source ? '<span class="badge">No cookies</span>' : ok ? `<span class="badge ok">${icon.check} Signed in</span>` : '<span class="badge warn">Signed out</span>';
  const when = r.source ? `Last refresh ${ago(r.last_refresh)}${r.next_check ? ` · next check ${until(r.next_check)}` : ''}` : '';
  return `<div class="row between wrap" style="gap:8px;align-items:flex-start">
      <div><div style="font-weight:650">${esc(r.source_label || 'Not signed in')}</div>${r.account ? `<div class="small muted">Account: ${esc(r.account)}</div>` : ''}</div>${badge}</div>
    ${when ? `<div class="small muted" style="margin-top:6px">${esc(when)}</div>` : ''}
    <div class="small muted row wrap" style="gap:4px 14px;margin-top:6px">${['managed', 'extension', 'pasted'].map(k => sourceLine(k, (r.sources || {})[k])).join('')}</div>
    <div class="small faint" style="margin-top:6px">${esc(r.diagnostics || '')}</div>`;
}

/** The token box shown after "Connect browser extension". */
export function extensionHtml(token) {
  return `<div class="card flat mt" style="padding:12px 14px">
    <div style="font-weight:650">Connect the Encore extension</div>
    <ol class="small muted" style="margin:6px 0 10px;padding-left:18px">
      <li>Install the extension: download Encore-youtube-extension from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json.</li>
      <li>Open its options, enter this Encore’s address and the token below, and save.</li>
      <li>Stay signed in to YouTube in that browser. The extension sends the cookies when they change and every 6 hours.</li>
    </ol>
    <div class="row wrap" style="gap:8px"><div class="code grow mono" data-yt-token>${esc(token)}</div><button class="btn sm" type="button" data-yt-copy>${icon.copy}Copy</button></div>
    <div class="small faint" style="margin-top:6px">Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one.</div>
  </div>`;
}

/** The live view: the browser's screen, typing box and buttons. */
export function liveHtml() {
  return `<div class="card flat mt" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px;margin-bottom:6px"><span class="small muted" data-live-status>Starting the browser…</span>
      <span class="row" style="gap:6px"><button class="btn sm" type="button" data-live="back">Back</button><button class="btn sm" type="button" data-live="reload">Reload</button><button class="btn sm" type="button" data-live="close">Close</button></span></div>
    <div class="small faint mono" data-live-url style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap"></div>
    <img data-live-screen alt="YouTube sign-in" tabindex="0" draggable="false" style="width:100%;max-width:1280px;border-radius:8px;border:1px solid var(--line);display:block;outline:none;touch-action:none;margin-top:6px;min-height:120px;background:var(--bg-2)">
    <input class="input" data-live-input placeholder="Type here to send text to the page (Enter submits)" autocomplete="off" autocapitalize="off" spellcheck="false" style="margin-top:8px">
    <div class="small muted" style="margin-top:6px">Click into the page and type, or use the box above. What you type goes to Google through Encore’s browser on this server; Encore keeps only the sign-in cookies.</div>
  </div>`;
}

const KEYS = ['Enter', 'Backspace', 'Tab', 'Escape', 'Delete', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'];

/** Wire the card: status, buttons, the extension token and the live view. */
export function bindYoutube(card) {
  const host = card.querySelector('#yt-auth');
  if (!host) return;
  const status = host.querySelector('[data-yt-status]');
  const extBox = host.querySelector('[data-yt-ext]');
  const liveBox = host.querySelector('[data-yt-live]');
  let info = null, ws = null;
  const paint = (r) => { info = r; status.innerHTML = youtubeStatusHtml(r); };
  const load = async () => { try { paint(await api.get('/youtube/auth')); } catch (e) { status.innerHTML = `<div class="small" style="color:var(--err)">${esc(e.message)}</div>`; } };
  const stopLive = () => { if (ws) { try { ws.close(); } catch { /* closed */ } ws = null; } liveBox.hidden = true; liveBox.innerHTML = ''; };

  const startLive = () => {
    stopLive();
    liveBox.hidden = false;
    liveBox.innerHTML = liveHtml();
    const img = liveBox.querySelector('[data-live-screen]'), say = liveBox.querySelector('[data-live-status]');
    const urlLine = liveBox.querySelector('[data-live-url]'), input = liveBox.querySelector('[data-live-input]');
    let size = { w: 1280, h: 860 }, lastMove = 0;
    const sock = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/youtube/browser/live`);
    ws = sock;
    const send = (m) => { if (sock.readyState === 1) sock.send(JSON.stringify(m)); };
    const at = (e) => { const b = img.getBoundingClientRect(); return { x: Math.round((e.clientX - b.left) / b.width * size.w), y: Math.round((e.clientY - b.top) / b.height * size.h) }; };
    sock.onmessage = (ev) => {
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (!img.isConnected) { stopLive(); return; }   // the page was left
      if (m.type === 'frame') { size = { w: m.width, h: m.height }; img.src = `data:image/jpeg;base64,${m.data}`; say.textContent = 'Sign in with your Google account'; }
      else if (m.type === 'url') urlLine.textContent = m.url || '';
      else if (m.type === 'status') say.textContent = m.message || '';
      else if (m.type === 'error') { say.textContent = m.message || 'The browser stopped'; toast(m.message || 'The browser stopped', 'err'); }
      else if (m.type === 'done') { toast(m.account ? `Signed in as ${m.account}` : 'Signed in to YouTube', 'ok'); stopLive(); load(); }
    };
    sock.onclose = () => { if (ws === sock) { say.textContent = 'The browser closed'; ws = null; } };
    img.addEventListener('pointerdown', (e) => { e.preventDefault(); img.focus(); send({ type: 'mouse', event: 'down', ...at(e), button: e.button === 2 ? 'right' : 'left', clickCount: 1 }); });
    img.addEventListener('pointerup', (e) => { e.preventDefault(); send({ type: 'mouse', event: 'up', ...at(e), button: e.button === 2 ? 'right' : 'left', clickCount: 1 }); });
    img.addEventListener('pointermove', (e) => { const t = Date.now(); if (t - lastMove < 80) return; lastMove = t; send({ type: 'mouse', event: 'move', ...at(e) }); });
    img.addEventListener('wheel', (e) => { e.preventDefault(); send({ type: 'mouse', event: 'wheel', ...at(e), deltaX: e.deltaX, deltaY: e.deltaY }); }, { passive: false });
    img.addEventListener('contextmenu', (e) => e.preventDefault());
    img.addEventListener('keydown', (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key.length === 1) { e.preventDefault(); send({ type: 'text', text: e.key }); }
      else if (KEYS.includes(e.key)) { e.preventDefault(); send({ type: 'key', key: e.key, code: e.code }); }
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') { e.preventDefault(); if (input.value) send({ type: 'text', text: input.value }); input.value = ''; send({ type: 'key', key: 'Enter', code: 'Enter' }); }
      else if (e.key === 'Backspace' && !input.value) { e.preventDefault(); send({ type: 'key', key: 'Backspace', code: 'Backspace' }); }
      else if (e.key === 'Tab') { e.preventDefault(); if (input.value) send({ type: 'text', text: input.value }); input.value = ''; send({ type: 'key', key: 'Tab', code: 'Tab' }); }
    });
    liveBox.querySelectorAll('[data-live]').forEach(b => b.addEventListener('click', () => {
      const a = b.dataset.live;
      if (a === 'close') stopLive();
      else send({ type: 'nav', action: a });
    }));
  };

  host.querySelectorAll('[data-yt]').forEach(b => b.addEventListener('click', async (e) => {
    e.preventDefault();
    const a = b.dataset.yt;
    if (a === 'signin') {
      if (info && !(info.browser || {}).installed) { toast('Install “Browser for YouTube sign-in” under Download tools first', 'err'); return; }
      startLive();
    } else if (a === 'refresh') {
      b.disabled = true;
      try { const r = await api.post('/youtube/refresh'); paint(r); toast(r.refreshed ? 'Refreshed – still signed in' : 'YouTube signed out', r.refreshed ? 'ok' : 'err'); } catch (err) { toast(err.message, 'err'); }
      b.disabled = false;
    } else if (a === 'signout') {
      if (!confirm('Sign Encore’s browser out of YouTube and forget the extension’s cookies? Pasted cookies stay (clear them below).')) return;
      stopLive();
      try { paint(await api.post('/youtube/signout', { source: 'all' })); toast('Signed out', 'ok'); } catch (err) { toast(err.message, 'err'); }
    } else if (a === 'extension') {
      if (info && (info.extension || {}).token_at && !confirm('Make a new token? The extension stops sending cookies until you enter the new one.')) return;
      try {
        const r = await api.post('/youtube/extension-token');
        extBox.hidden = false;
        extBox.innerHTML = extensionHtml(r.token);
        extBox.querySelector('[data-yt-copy]').onclick = () => copyText(r.token);
        load();
      } catch (err) { toast(err.message, 'err'); }
    }
  }));
  load();
}
