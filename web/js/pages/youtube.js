// Settings → YouTube sign-in: which cookies yt-dlp uses (a sign-in handed over by the Encore browser
// extension, the extension's sync of an everyday browser, or pasted cookies), Refresh now / Sign out, and
// the extension token. Mirrored in web-rs/src/pages/youtube.rs (same words, same DOM).
import { api } from '../api.js';
import { copyText, esc, icon, toast } from '../ui.js';

const ago = (t) => { if (!t) return 'never'; const s = Date.now() / 1000 - t; return s < 90 ? 'just now' : s < 5400 ? `${Math.round(s / 60)} min ago` : s < 129600 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} days ago`; };
const until = (t) => { if (!t) return ''; const s = t - Date.now() / 1000; return s <= 60 ? 'any moment' : s < 5400 ? `in ${Math.round(s / 60)} min` : s < 129600 ? `in ${Math.round(s / 3600)} h` : `in ${Math.round(s / 86400)} days`; };

/** The card's frame; the status inside is filled in by bindYoutube. */
export function youtubeBlock() {
  return `<div class="card flat" id="yt-auth" style="padding:12px 14px;margin-bottom:14px">
    <div data-yt-status><div class="small muted">Loading…</div></div>
    <div class="row wrap mt" style="gap:8px">
      <button class="btn sm primary" type="button" data-yt="extension">${icon.link}Connect the browser extension</button>
      <button class="btn sm" type="button" data-yt="refresh">${icon.refresh}Refresh now</button>
      <button class="btn sm danger" type="button" data-yt="signout">${icon.x}Sign out</button>
    </div>
    <div data-yt-ext hidden></div>
  </div>`;
}

const SOURCE_WORDS = { handoff: 'Handed-over sign-in', extension: 'Everyday browser (extension)', pasted: 'Pasted cookies' };

function sourceLine(key, s) {
  let state;
  if (!s || !s.has_cookies) state = key === 'pasted' ? 'none' : 'not connected';
  else if (s.signed_in) state = `signed in${s.account ? ` as ${s.account}` : ''}`;
  else state = 'signed out';
  return `<span>${esc(SOURCE_WORDS[key])}: ${esc(state)}</span>`;
}

/** The status part of the card (from GET /api/youtube/auth). */
export function youtubeStatusHtml(r) {
  const src = r.source ? r.sources[r.source] : null;
  const ok = !!(src && src.signed_in);
  const badge = !r.source ? '<span class="badge">No cookies</span>' : ok ? `<span class="badge ok">${icon.check} Signed in</span>` : '<span class="badge warn">Signed out</span>';
  const next = r.next_refresh || r.next_check;
  const when = r.source ? `Last refresh ${ago(r.last_refresh)}${next ? ` · next refresh ${until(next)}` : ''}` : '';
  return `<div class="row between wrap" style="gap:8px;align-items:flex-start">
      <div><div style="font-weight:650">${esc(r.source_label || 'Not signed in')}</div>${r.account ? `<div class="small muted">Account: ${esc(r.account)}</div>` : ''}</div>${badge}</div>
    ${when ? `<div class="small muted" style="margin-top:6px">${esc(when)}</div>` : ''}
    <div class="small muted row wrap" style="gap:4px 14px;margin-top:6px">${['handoff', 'extension', 'pasted'].map(k => sourceLine(k, (r.sources || {})[k])).join('')}</div>
    <div class="small faint" style="margin-top:6px">${esc(r.diagnostics || '')}</div>`;
}

/** The token box shown after "Connect the browser extension". */
export function extensionHtml(token) {
  return `<div class="card flat mt" style="padding:12px 14px">
    <div style="font-weight:650">Connect the Encore extension</div>
    <ol class="small muted" style="margin:6px 0 10px;padding-left:18px">
      <li>Install it on a computer: download Encore-youtube-extension-&lt;version&gt;.zip from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder, then Details → Allow in Incognito. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json, then about:addons → Run in Private Windows → Allow.</li>
      <li>Open its options and paste this Encore’s address and the token below.</li>
      <li>Press <b>Hand over a YouTube sign-in</b> in the extension. A private window opens: sign in to Google there. Once YouTube Music shows you signed in, the extension sends the sign-in to Encore and closes the window without signing out.</li>
    </ol>
    <div class="row wrap" style="gap:8px"><div class="code grow mono" data-yt-token>${esc(token)}</div><button class="btn sm" type="button" data-yt-copy>${icon.copy}Copy</button></div>
    <div class="small faint" style="margin-top:6px">Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one. After the hand-off Encore owns that sign-in and refreshes it itself every 4 hours, also while your computer is off.</div>
  </div>`;
}

/** Wire the card: status, buttons and the extension token. */
export function bindYoutube(card) {
  const host = card.querySelector('#yt-auth');
  if (!host) return;
  const status = host.querySelector('[data-yt-status]');
  const extBox = host.querySelector('[data-yt-ext]');
  let info = null;
  const paint = (r) => { info = r; status.innerHTML = youtubeStatusHtml(r); };
  const load = async () => { try { paint(await api.get('/youtube/auth')); } catch (e) { status.innerHTML = `<div class="small" style="color:var(--err)">${esc(e.message)}</div>`; } };

  host.querySelectorAll('[data-yt]').forEach(b => b.addEventListener('click', async (e) => {
    e.preventDefault();
    const a = b.dataset.yt;
    if (a === 'refresh') {
      b.disabled = true;
      try { const r = await api.post('/youtube/refresh'); paint(r); toast(r.refreshed ? 'Refreshed – still signed in' : 'YouTube signed out', r.refreshed ? 'ok' : 'err'); } catch (err) { toast(err.message, 'err'); }
      b.disabled = false;
    } else if (a === 'signout') {
      if (!confirm('Forget the handed-over sign-in and the extension’s cookies? Pasted cookies stay (clear them below).')) return;
      try { paint(await api.post('/youtube/signout', { source: 'all' })); toast('Signed out', 'ok'); } catch (err) { toast(err.message, 'err'); }
    } else if (a === 'extension') {
      if (info && (info.extension || {}).token_at && !confirm('Make a new token? The extension stops working until you enter the new one in its options.')) return;
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
