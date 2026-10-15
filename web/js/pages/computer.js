// Settings → This computer: only in the Encore desktop app (window.EncoreNative.desktopSettings, clients/app/src/extras.rs).
// Start with the computer, the mini player, global keyboard shortcuts, offline songs and updates.
// The same words in web-rs/src/pages/computer.rs.
import { esc, toast } from '../ui.js';

export const KEYS = [['toggle', 'Play / pause'], ['next', 'Next song'], ['prev', 'Previous song'], ['volume_up', 'Volume up'], ['volume_down', 'Volume down']];

const parse = (v) => (typeof v === 'string' ? JSON.parse(v) : v) || {};

/** A key press as a shortcut ("Ctrl+Alt+P"): '' clears (Backspace / Delete alone), null while only modifiers are down. */
export function accelerator(e) {
  const mods = e.ctrlKey || e.altKey || e.shiftKey || e.metaKey;
  if ((e.key === 'Backspace' || e.key === 'Delete') && !mods) return '';
  if (['Control', 'Alt', 'Shift', 'Meta', 'OS', 'AltGraph'].includes(e.key) || !e.code) return null;
  const parts = [];
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  if (e.metaKey) parts.push('Super');
  const code = e.code.replace(/^Key([A-Z])$/, '$1').replace(/^Digit([0-9])$/, '$1').replace(/^Arrow(Up|Down|Left|Right)$/, '$1');
  parts.push(code);
  return parts.join('+');
}

/** The page from the app's settings `s`, the last update check `upd` (null = none this visit) and whether a button is working. */
export function pageHtml(s, upd, busy) {
  const h = s.hotkeys || {};
  const errs = s.hotkey_errors || {};
  const tray = s.platform === 'macos' ? 'menu bar' : 'tray';
  const keys = KEYS.map(([k, l]) => `<label class="stack" style="gap:4px"><span class="small muted">${esc(l)}</span><input class="input mono" data-key="${k}" value="${esc(h[k] || '')}" placeholder="Not set" readonly ${h.on ? '' : 'disabled'}>${errs[k] ? `<span class="small" style="color:var(--err)">${esc(errs[k])}</span>` : ''}</label>`).join('');
  const version = upd?.available ? upd.version : (upd ? '' : (s.update || ''));
  const line = busy ? 'Working…'
    : upd?.error ? upd.error
    : version ? `Encore ${version} is ready to install.`
    : upd ? 'Encore is up to date.' : '';
  const updates = s.updates
    ? `<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-autoupdate" ${s.check_updates ? 'checked' : ''}> Look for a new version when Encore starts</label>
      ${line ? `<span class="small ${upd?.error ? '' : 'muted'}" ${upd?.error ? 'style="color:var(--err)"' : ''}>${esc(line)}</span>` : ''}
      <div class="row wrap"><button type="button" class="btn sm" id="pc-check" ${busy ? 'disabled' : ''}>Check for updates</button>${version ? `<button type="button" class="btn sm primary" id="pc-install" ${busy ? 'disabled' : ''}>Install ${esc(version)} and restart</button>` : ''}</div>`
    : '<span class="small muted">This build can\'t update itself. New versions are on <a href="https://github.com/Krifmaz/Encore/releases" target="_blank" rel="noopener">github.com/Krifmaz/Encore/releases</a>.</span>';
  return `<div class="card stack">
    <h2>This computer</h2>
    <p class="small muted" style="margin:0">Settings of the Encore app on this computer. They stay on this computer and are not saved to your account.</p>
    <div class="stack" style="gap:6px"><h3>Start</h3>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-autostart" ${s.autostart ? 'checked' : ''}> Start Encore when I sign in to this computer</label>
      <span class="small muted">It starts in the ${tray} without a window. Click the icon to open it.</span></div>
    <div class="stack" style="gap:6px"><h3>Mini player</h3>
      <div class="row"><button type="button" class="btn sm" id="pc-mini">Open or close the mini player</button></div>
      <span class="small muted">A small window that stays on top of the others, with the cover, the song, the buttons and the seek bar. Also in the ${tray} menu and in the player's ⋯ menu.</span></div>
    <div class="stack" style="gap:6px"><h3>Keyboard shortcuts</h3>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pc-keys" ${h.on ? 'checked' : ''}> Use these shortcuts in every program</label>
      <div class="stack" style="gap:8px;${h.on ? '' : 'opacity:.45'}">${keys}</div>
      <span class="small muted">Click a box and press the keys you want, for example Ctrl+Alt+P. Backspace clears it. The media keys on your keyboard always work.</span>
      <div class="row"><button type="button" class="btn sm" id="pc-keys-reset">Back to the standard shortcuts</button></div></div>
    <div class="stack" style="gap:6px"><h3>Offline music</h3>
      <span class="small muted">${s.offline ? `${s.offline} song(s) saved on this computer.` : 'No songs saved on this computer yet.'} Save songs, albums and playlists with ⋯ → Save for offline. When the server can't be reached, they play through Cadenza.</span>
      <div class="row"><button type="button" class="btn sm" id="pc-offline" ${s.offline ? '' : 'disabled'}>Open offline music</button></div></div>
    <div class="stack" style="gap:6px"><h3>Updates</h3>
      <span class="small muted">Version ${esc(s.version || '?')}</span>
      ${updates}</div>
  </div>`;
}

export async function render(el) {
  const n = window.EncoreNative;
  if (!n?.desktopSettings) {
    el.innerHTML = '<div class="card stack"><h2>This computer</h2><p class="small muted" style="margin:0">These settings are in the Encore desktop app.</p></div>';
    return;
  }
  let s;
  try { s = parse(await n.desktopSettings()); } catch (err) { el.innerHTML = `<div class="error-box">${esc(String(err?.message || err))}</div>`; return; }
  let upd = null;
  let busy = false;
  const set = async (patch) => {
    try { s = parse(await n.setDesktopSettings(JSON.stringify(patch))); toast('Saved', 'ok'); } catch (err) { toast(String(err?.message || err), 'err'); }
    paint();
  };
  const paint = () => {
    el.innerHTML = pageHtml(s, upd, busy);
    const h = s.hotkeys || {};
    el.querySelector('#pc-autostart').onchange = (e) => set({ autostart: e.target.checked });
    el.querySelector('#pc-mini').onclick = () => n.miniPlayer();
    el.querySelector('#pc-keys').onchange = (e) => set({ hotkeys: { ...h, on: e.target.checked } });
    el.querySelector('#pc-keys-reset').onclick = () => set({ hotkeys: s.default_hotkeys });
    el.querySelectorAll('[data-key]').forEach(box => box.addEventListener('keydown', (e) => {
      if (e.key === 'Tab' && !e.ctrlKey && !e.altKey && !e.metaKey) return;
      e.preventDefault();
      const acc = accelerator(e);
      if (acc === null || acc === (h[box.dataset.key] || '')) return;
      set({ hotkeys: { ...h, [box.dataset.key]: acc } });
    }));
    el.querySelector('#pc-offline').onclick = () => n.openOffline();
    const auto = el.querySelector('#pc-autoupdate');
    if (auto) auto.onchange = (e) => set({ check_updates: e.target.checked });
    const check = el.querySelector('#pc-check');
    if (check) check.onclick = async () => {
      busy = true; paint();
      try { upd = parse(await n.checkUpdate()); } catch (err) { upd = { error: String(err?.message || err) }; }
      busy = false; paint();
    };
    const install = el.querySelector('#pc-install');
    if (install) install.onclick = async () => {
      busy = true; paint();
      let r;
      try { r = parse(await n.installUpdate()); } catch (err) { r = { ok: false, error: String(err?.message || err) }; }
      // on success the app restarts and this page is gone
      busy = false;
      if (!r.ok) { upd = { error: r.error || 'The update didn\'t install.' }; toast(upd.error, 'err'); }
      paint();
    };
  };
  paint();
}
