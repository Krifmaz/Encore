// Translated titles: Settings → Language and ⋯ → Edit translation (the same words in web-rs/src/pages/translate.rs).
import { api } from '../api.js';
import { esc, icon, modal, setTranslateMode, toast, translateMode } from '../ui.js';

const KIND_LABEL = { title: 'Song title', album: 'Album title', artist: 'Artist name' };

/** The ⋯ menu entry for one text (null when translations are off or there is nothing to edit). */
export function translateMenuItem(text, kind, onSaved) {
  if (translateMode() === 'off' || !text) return null;
  return { label: 'Edit translation', icon: icon.edit, onClick: () => editTranslation(text, kind, onSaved) };
}

/** Correct the translation and romanization of one title (shared with everyone who reads the same language). */
export async function editTranslation(text, kind = 'title', onSaved) {
  let e;
  try { e = await api.get(`/translate/entry?text=${encodeURIComponent(text)}`); } catch (err) { toast(err.message, 'err'); return; }
  const source = { official: 'Official (MusicBrainz or Apple)', model: 'The Translator on this server', manual: 'Corrected by hand' }[e.source] || 'Not translated yet';
  const m = modal({
    title: 'Edit translation',
    body: `<div class="stack" style="gap:12px">
      <div><div class="small muted">${esc(KIND_LABEL[kind] || KIND_LABEL.title)}</div><div style="font-weight:650">${esc(e.text)}</div></div>
      <label class="field"><span>Translation</span><input class="input" id="tr-t" value="${esc(e.translation || '')}" placeholder="Leave empty to show none"></label>
      <label class="field"><span>Romanization</span><input class="input" id="tr-r" value="${esc(e.romanization || '')}" placeholder="Leave empty to show none"></label>
      <div class="small muted">Now from: ${esc(source)}. Your correction is kept for everyone who reads this language and is never replaced by the automatic translation.</div>
    </div>`,
    footer: `${e.manual ? '<button class="btn" id="tr-reset">Use the automatic translation</button>' : ''}<button class="btn" data-close>Cancel</button><button class="btn primary" id="tr-save">${icon.check}Save</button>`,
  });
  const done = (msg) => { m.close(); toast(msg, 'ok'); onSaved?.(); };
  m.el.querySelector('#tr-save').onclick = async () => {
    try {
      await api.put('/translate/entry', { text: e.text, kind, translation: m.el.querySelector('#tr-t').value.trim(), romanization: m.el.querySelector('#tr-r').value.trim() });
      done('Translation saved');
    } catch (err) { toast(err.message, 'err'); }
  };
  m.el.querySelector('#tr-reset')?.addEventListener('click', async () => {
    try { await api.del(`/translate/entry?text=${encodeURIComponent(e.text)}`); done('Back to the automatic translation'); } catch (err) { toast(err.message, 'err'); }
  });
}

/** Settings → Language: how translated titles show, and into which language. */
export async function renderLanguage(el) {
  const { session } = await import('../app.js');
  let opts, status = {};
  try { opts = await api.get('/translate/languages'); } catch (err) { el.innerHTML = `<div class="error-box">${esc(err.message)}</div>`; return; }
  try { status = await api.get('/translate/status'); } catch { /* shown without the status line */ }
  const prefs = session.user?.prefs || {};
  let mode = prefs.translate_mode || 'off', lang = prefs.translate_lang || opts.default || 'en';
  const paint = () => {
    el.innerHTML = `<div class="card stack">
      <h2>Language</h2>
      <p class="small muted" style="margin:0">Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car.</p>
      <div class="stack" style="gap:6px"><h3>Translated titles</h3>
        <div class="seg" data-key="mode">${opts.modes.map(x => `<button type="button" data-v="${esc(x.id)}" class="${mode === x.id ? 'on' : ''}">${esc(x.label)}</button>`).join('')}</div></div>
      <label class="stack" style="gap:4px"><span class="small muted">Translate into</span>
        <select class="input" id="tr-lang" ${mode === 'off' ? 'disabled' : ''}>${opts.languages.map(l => `<option value="${esc(l.code)}" ${l.code === lang ? 'selected' : ''}>${esc(l.name)}</option>`).join('')}</select></label>
      <div class="small muted">Official translations come first (MusicBrainz and Apple, where the label published one); the rest come from the Translator on this server. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation.</div>
      ${status.translator === false ? '<div class="small" style="color:var(--warn)">The Translator is not installed on this server, so only official translations show. An admin can install it under Settings → Download tools.</div>' : ''}
      ${status.rows != null ? `<div class="small faint">${status.shown || 0} of ${status.rows || 0} titles have a translation · ${status.waiting || 0} waiting for the Translator${status.running ? ' · running now' : ''}</div>` : ''}
    </div>`;
    el.querySelectorAll('.seg[data-key="mode"] button').forEach(b => b.onclick = () => save({ translate_mode: b.dataset.v }));
    el.querySelector('#tr-lang').onchange = (e) => save({ translate_lang: e.target.value });
  };
  const save = async (patch) => {
    try {
      const p = await api.put('/auth/prefs', patch);
      if (session.user) session.user.prefs = p;
      mode = p.translate_mode || 'off'; lang = p.translate_lang || lang;
      setTranslateMode(mode);
      api.post('/translate/run').catch(() => {});
      toast('Saved', 'ok');
    } catch (err) { toast(err.message, 'err'); }
    paint();
  };
  paint();
}
