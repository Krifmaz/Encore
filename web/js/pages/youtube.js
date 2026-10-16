// Settings → YouTube sign-in: which cookies yt-dlp uses (Encore's browser, the browser extension or pasted
// cookies), the guided sign-in (Encore's own form in front of Google's pages) with the live view as the
// fallback, Refresh now / Sign out, and the extension token.
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
    <div data-yt-guided hidden></div>
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

/** One step of the guided sign-in (from POST /api/youtube/guided/start or /step). */
export function guidedHtml(s) {
  const err = s.error ? `<div class="small" data-g-error style="color:var(--err);margin-top:6px">${esc(s.error)}</div>` : '';
  const text = s.text ? `<div class="small muted" style="margin-top:4px;white-space:pre-line">${esc(s.text)}</div>` : '';
  const choices = (s.choices || []).map(c => `<button class="btn sm${c.default ? ' primary' : ''}" type="button" data-g-choice="${esc(c.id)}" style="justify-content:flex-start;text-align:left;white-space:normal">${esc(c.label)}</button>`).join('');
  const list = choices ? `<div style="display:flex;flex-direction:column;gap:6px;margin-top:10px">${choices}</div>` : '';
  const cancel = '<button class="btn sm" type="button" data-g="cancel">Cancel</button>';
  const live = '<button class="btn sm" type="button" data-g="live">Open the live view</button>';
  const check = '<button class="btn sm" type="button" data-g="check">Check again</button>';
  let body;
  if (s.kind === 'email') {
    body = `<form data-g-form style="margin-top:10px"><input class="input" data-g-input type="email" name="username" autocomplete="username" inputmode="email" autocapitalize="off" spellcheck="false" placeholder="Email or phone" required>${err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Next</button>${cancel}</div></form>`;
  } else if (s.kind === 'password') {
    body = `<form data-g-form style="margin-top:10px"><div class="row" style="gap:8px"><input class="input grow" data-g-input type="password" name="password" autocomplete="current-password" autocapitalize="off" spellcheck="false" placeholder="Password" required><button class="btn sm" type="button" data-g="show">Show</button></div>${err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Next</button>${cancel}</div></form>`;
  } else if (s.kind === 'code') {
    const field = s.code_hint === 'totp'
      ? '<input class="input" data-g-input type="text" name="code" inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]*" maxlength="9" placeholder="6-digit code" required>'
      : '<input class="input" data-g-input type="text" name="code" autocomplete="one-time-code" autocapitalize="off" spellcheck="false" placeholder="Code" required>';
    body = `<form data-g-form style="margin-top:10px">${field}${err}
      <div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="submit">Verify</button>${cancel}</div></form>${list}`;
  } else if (s.kind === 'choice') {
    body = `${err}${list}<div class="row wrap" style="gap:8px;margin-top:10px">${cancel}</div>`;
  } else if (s.kind === 'wait') {
    body = `${err}${list}<div class="row wrap" style="gap:8px;margin-top:10px">${check}${cancel}</div>`;
  } else if (s.kind === 'error') {
    body = `${err}<div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="button" data-g="restart">Start again</button>${live}<button class="btn sm" type="button" data-g="close">Close</button></div>`;
  } else {
    const shot = s.screenshot ? `<img data-g-shot alt="Google’s page" src="data:image/jpeg;base64,${esc(s.screenshot)}" style="width:100%;max-width:640px;border-radius:8px;border:1px solid var(--line);display:block;margin-top:10px">` : '';
    body = `${err}${shot}${list}<div class="row wrap" style="gap:8px;margin-top:10px"><button class="btn sm primary" type="button" data-g="live">Open the live view</button>${check}${cancel}</div>`;
  }
  return `<div class="card flat mt" style="padding:12px 14px" data-g-kind="${esc(s.kind || '')}">
    <div style="font-weight:650">${esc(s.title || 'Sign in to YouTube')}</div>${text}${body}
    <div class="small faint" style="margin-top:10px">Encore types this into Google’s sign-in in its own browser on this server and keeps only the sign-in cookies. Security keys and passkeys can’t be used there: choose the authenticator app instead.</div>
  </div>`;
}

/** The guided sign-in while it waits for the browser or Google. */
export function guidedBusyHtml(message) {
  return `<div class="card flat mt" style="padding:12px 14px"><div class="small muted" data-g-busy>${esc(message)}</div></div>`;
}

/** The live view: the browser's screen, typing box and buttons. */
export function liveHtml() {
  return `<div class="card flat mt" style="padding:12px 14px">
    <div class="row between wrap" style="gap:8px;margin-bottom:6px"><span class="small muted" data-live-status>Starting the browser…</span>
      <span class="row" style="gap:6px"><button class="btn sm" type="button" data-live="back">Back</button><button class="btn sm" type="button" data-live="reload">Reload</button><button class="btn sm" type="button" data-live="close">Close</button></span></div>
    <div class="small faint mono" data-live-url style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap"></div>
    <img data-live-screen alt="YouTube sign-in" tabindex="0" draggable="false" style="width:100%;max-width:1280px;border-radius:8px;border:1px solid var(--line);display:block;outline:none;touch-action:none;object-fit:contain;margin-top:6px;min-height:120px;background:var(--bg-2)">
    <form data-live-form style="margin-top:8px"><input class="input" data-live-input placeholder="Tap a field in the picture, then type here" autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck="false" enterkeyhint="send"></form>
    <div class="row wrap" style="gap:6px;margin-top:6px"><button class="btn sm" type="button" data-live-key="Enter">Enter</button><button class="btn sm" type="button" data-live-key="Tab">Tab</button><button class="btn sm" type="button" data-live-key="Backspace">Backspace</button></div>
    <div class="small muted" style="margin-top:6px">Tap or click a field in the picture, then type in the box: the text goes to the page as you type. What you type goes to Google through Encore’s browser on this server; Encore keeps only the sign-in cookies.</div>
  </div>`;
}

/** A point on the shown picture → the browser's own pixels. `box` is the image's content box (CSS
 *  pixels, like clientX/Y, so the device pixel ratio cancels out); the picture keeps its shape inside it
 *  (object-fit: contain), so bars at the sides or top are left out. */
export function mapPoint(cx, cy, box, w, h) {
  const scale = Math.min(box.width / w, box.height / h) || 1;
  const left = box.left + (box.width - w * scale) / 2, top = box.top + (box.height - h * scale) / 2;
  const clamp = (v, max) => Math.min(max - 1, Math.max(0, Math.round(v)));
  return { x: clamp((cx - left) / scale, w), y: clamp((cy - top) / scale, h) };
}

const KEYS = ['Enter', 'Backspace', 'Tab', 'Escape', 'Delete', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'];
const G_FAILED = 'The sign-in stopped';

/** Wire the card: status, buttons, the extension token and the live view. */
export function bindYoutube(card) {
  const host = card.querySelector('#yt-auth');
  if (!host) return;
  const status = host.querySelector('[data-yt-status]');
  const extBox = host.querySelector('[data-yt-ext]');
  const liveBox = host.querySelector('[data-yt-live]');
  const gBox = host.querySelector('[data-yt-guided]');
  let info = null, ws = null, gen = 0;
  const paint = (r) => { info = r; status.innerHTML = youtubeStatusHtml(r); };
  const load = async () => { try { paint(await api.get('/youtube/auth')); } catch (e) { status.innerHTML = `<div class="small" style="color:var(--err)">${esc(e.message)}</div>`; } };
  const stopLive = () => { if (ws) { try { ws.close(); } catch { /* closed */ } ws = null; } liveBox.hidden = true; liveBox.innerHTML = ''; };

  // ---- guided sign-in
  const gHide = () => { gBox.hidden = true; gBox.innerHTML = ''; };
  const gBusy = (msg) => { gBox.hidden = false; gBox.innerHTML = guidedBusyHtml(msg); };
  const gFail = (e) => ({ kind: 'error', title: G_FAILED, error: e.message });
  const gShow = (s) => {
    if (!gBox.isConnected) return;
    if (s.kind === 'done') { gen++; gHide(); toast(s.account ? `Signed in as ${s.account}` : 'Signed in to YouTube', 'ok'); load(); return; }
    gBox.hidden = false;
    gBox.innerHTML = guidedHtml(s);
    const input = gBox.querySelector('[data-g-input]'), form = gBox.querySelector('[data-g-form]');
    if (input) input.focus();
    if (form) form.addEventListener('submit', (e) => {
      e.preventDefault();
      const v = input.value;
      if (!v.trim()) return;
      input.value = '';
      gSend({ kind: s.kind, value: v });
    });
    gBox.querySelectorAll('[data-g-choice]').forEach(b => b.addEventListener('click', () => gSend({ kind: 'choice', value: b.dataset.gChoice })));
    gBox.querySelectorAll('[data-g]').forEach(b => b.addEventListener('click', () => {
      const a = b.dataset.g;
      if (a === 'cancel' || a === 'close') gCancel();
      else if (a === 'check') gSend({ kind: 'check' });
      else if (a === 'restart') gStart();
      else if (a === 'live') { gHide(); startLive(s.kind !== 'error'); }
      else if (a === 'show') { const shown = input.type === 'password'; input.type = shown ? 'text' : 'password'; b.textContent = shown ? 'Hide' : 'Show'; input.focus(); }
    }));
  };
  const gSend = async (body) => {
    const mine = ++gen;
    gBusy('Waiting for Google…');
    let s;
    try { s = await api.post('/youtube/guided/step', body); } catch (e) { s = gFail(e); }
    if (mine === gen) gShow(s);
  };
  const gStart = async () => {
    stopLive();
    const mine = ++gen;
    gBusy('Starting the browser…');
    let s;
    try { s = await api.post('/youtube/guided/start'); } catch (e) { s = gFail(e); }
    if (mine === gen) gShow(s);
  };
  const gCancel = () => { gen++; gHide(); api.post('/youtube/guided/cancel').catch(() => { /* closed already */ }); };

  // ---- live view (fallback): `guided` when it shows the guided sign-in's browser
  const startLive = (guided) => {
    stopLive();
    liveBox.hidden = false;
    liveBox.innerHTML = liveHtml();
    const img = liveBox.querySelector('[data-live-screen]'), say = liveBox.querySelector('[data-live-status]');
    const urlLine = liveBox.querySelector('[data-live-url]'), input = liveBox.querySelector('[data-live-input]');
    let size = { w: 1280, h: 860 }, lastMove = 0, sent = '', timer = 0, touch = null;
    const sock = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/youtube/browser/live`);
    ws = sock;
    const send = (m) => { if (sock.readyState === 1) sock.send(JSON.stringify(m)); };
    const at = (x, y) => { const r = img.getBoundingClientRect(); return mapPoint(x, y, { left: r.left + img.clientLeft, top: r.top + img.clientTop, width: img.clientWidth, height: img.clientHeight }, size.w, size.h); };
    const button = (e) => e.button === 2 ? 'right' : 'left';
    const click = (p) => { send({ type: 'mouse', event: 'move', ...p }); send({ type: 'mouse', event: 'down', ...p, button: 'left', clickCount: 1 }); send({ type: 'mouse', event: 'up', ...p, button: 'left', clickCount: 1 }); };
    // the box's text goes to the page as it changes (Android keyboards send no usable key events)
    const flush = () => {
      clearTimeout(timer); timer = 0;
      const was = Array.from(sent), now = Array.from(input.value);
      let i = 0;
      while (i < was.length && i < now.length && was[i] === now[i]) i++;
      for (let n = was.length - i; n > 0; n--) send({ type: 'key', key: 'Backspace', code: 'Backspace' });
      if (now.length > i) send({ type: 'text', text: now.slice(i).join('') });
      sent = input.value;
    };
    const press = (key) => {
      flush();
      send({ type: 'key', key, code: key });
      if (key === 'Backspace') { if (input.value) { input.value = Array.from(input.value).slice(0, -1).join(''); sent = input.value; } }
      else { input.value = ''; sent = ''; }
    };
    sock.onmessage = (ev) => {
      let m; try { m = JSON.parse(ev.data); } catch { return; }
      if (!img.isConnected) { stopLive(); return; }   // the page was left
      if (m.type === 'frame') { size = { w: m.width, h: m.height }; img.src = `data:image/jpeg;base64,${m.data}`; say.textContent = 'Sign in with your Google account'; }
      else if (m.type === 'guided') guided = true;   // the server attached the guided sign-in's browser
      else if (m.type === 'url') urlLine.textContent = m.url || '';
      else if (m.type === 'status') say.textContent = m.message || '';
      else if (m.type === 'error') { say.textContent = m.message || 'The browser stopped'; toast(m.message || 'The browser stopped', 'err'); }
      else if (m.type === 'done') { gen++; gHide(); toast(m.account ? `Signed in as ${m.account}` : 'Signed in to YouTube', 'ok'); stopLive(); load(); }
    };
    sock.onclose = () => { if (ws === sock) { say.textContent = 'The browser closed'; ws = null; } };
    // mouse and pen; touch is below (a tap is a click, a drag scrolls)
    img.addEventListener('pointerdown', (e) => { if (e.pointerType === 'touch') return; e.preventDefault(); flush(); img.focus(); send({ type: 'mouse', event: 'down', ...at(e.clientX, e.clientY), button: button(e), clickCount: 1 }); });
    img.addEventListener('pointerup', (e) => { if (e.pointerType === 'touch') return; e.preventDefault(); send({ type: 'mouse', event: 'up', ...at(e.clientX, e.clientY), button: button(e), clickCount: 1 }); });
    img.addEventListener('pointermove', (e) => { if (e.pointerType === 'touch') return; const t = Date.now(); if (t - lastMove < 80) return; lastMove = t; send({ type: 'mouse', event: 'move', ...at(e.clientX, e.clientY) }); });
    img.addEventListener('wheel', (e) => { e.preventDefault(); send({ type: 'mouse', event: 'wheel', ...at(e.clientX, e.clientY), deltaX: e.deltaX, deltaY: e.deltaY }); }, { passive: false });
    img.addEventListener('touchstart', (e) => {
      if (e.touches.length !== 1) { touch = null; return; }
      e.preventDefault();
      const t = e.touches[0];
      touch = { x: t.clientX, y: t.clientY, lastX: t.clientX, lastY: t.clientY, drag: false };
    }, { passive: false });
    img.addEventListener('touchmove', (e) => {
      if (!touch || e.touches.length !== 1) return;
      e.preventDefault();
      const t = e.touches[0];
      if (!touch.drag && Math.hypot(t.clientX - touch.x, t.clientY - touch.y) > 10) touch.drag = true;
      if (!touch.drag) return;
      const k = size.w / (img.clientWidth || size.w);
      send({ type: 'mouse', event: 'wheel', ...at(t.clientX, t.clientY), deltaX: (touch.lastX - t.clientX) * k, deltaY: (touch.lastY - t.clientY) * k });
      touch.lastX = t.clientX; touch.lastY = t.clientY;
    }, { passive: false });
    img.addEventListener('touchend', (e) => {
      if (!touch) return;
      e.preventDefault();
      const t = e.changedTouches[0];
      if (!touch.drag && t) { flush(); click(at(t.clientX, t.clientY)); }
      touch = null;
    }, { passive: false });
    img.addEventListener('touchcancel', () => { touch = null; });
    img.addEventListener('contextmenu', (e) => e.preventDefault());
    img.addEventListener('keydown', (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key.length === 1) { e.preventDefault(); send({ type: 'text', text: e.key }); }
      else if (KEYS.includes(e.key)) { e.preventDefault(); send({ type: 'key', key: e.key, code: e.code }); }
    });
    input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(flush, 300); });
    liveBox.querySelector('[data-live-form]').addEventListener('submit', (e) => { e.preventDefault(); press('Enter'); });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Tab') { e.preventDefault(); press('Tab'); }
      else if (e.key === 'Backspace' && !input.value) { e.preventDefault(); press('Backspace'); }
    });
    liveBox.querySelectorAll('[data-live-key]').forEach(b => {
      // keep the focus (and the phone's keyboard) in the box
      b.addEventListener('mousedown', (e) => e.preventDefault());
      b.addEventListener('click', () => { press(b.dataset.liveKey); if (input.value === '') input.focus(); });
    });
    liveBox.querySelectorAll('[data-live]').forEach(b => b.addEventListener('click', () => {
      const a = b.dataset.live;
      if (a === 'close') { stopLive(); if (guided) gSend({ kind: 'check' }); }
      else send({ type: 'nav', action: a });
    }));
  };

  host.querySelectorAll('[data-yt]').forEach(b => b.addEventListener('click', async (e) => {
    e.preventDefault();
    const a = b.dataset.yt;
    if (a === 'signin') {
      if (info && !(info.browser || {}).installed) { toast('Install “Browser for YouTube sign-in” under Download tools first', 'err'); return; }
      gStart();
    } else if (a === 'refresh') {
      b.disabled = true;
      try { const r = await api.post('/youtube/refresh'); paint(r); toast(r.refreshed ? 'Refreshed – still signed in' : 'YouTube signed out', r.refreshed ? 'ok' : 'err'); } catch (err) { toast(err.message, 'err'); }
      b.disabled = false;
    } else if (a === 'signout') {
      if (!confirm('Sign Encore’s browser out of YouTube and forget the extension’s cookies? Pasted cookies stay (clear them below).')) return;
      stopLive();
      gen++; gHide();
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
