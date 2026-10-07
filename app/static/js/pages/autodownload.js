import { api } from '../api.js';
import { esc, fmt, icon, toast } from '../ui.js';

export async function render(view) {
  let d = await api.get('/admin/autodownload');
  const s = d.settings;
  const sw = (k, label, help = '') => `<label class="switch" style="align-items:flex-start"><input type="checkbox" data-k="${k}" ${s[k] ? 'checked' : ''}><i style="margin-top:2px"></i>
    <span><b>${label}</b>${help ? `<br><span class="small muted">${help}</span>` : ''}</span></label>`;
  view.innerHTML = `
    <div class="row wrap between" style="margin-bottom:18px">
      <div><h1>Auto-download</h1><div class="small muted">Decide what Encore downloads on its own, how it picks quality, and for whom.</div></div>
      <div class="row"><button class="btn" id="run">${icon.refresh}Run now</button><button class="btn primary" id="save">${icon.check}Save</button></div>
    </div>
    <div class="stats" style="margin-bottom:20px">
      <div class="stat"><b>${fmt.num(d.stats.auto_active)}</b><span>Auto-downloads in progress</span></div>
      <div class="stat"><b>${fmt.num(d.stats.album_waiting)}</b><span>Songs waiting on their album</span></div>
      <div class="stat"><b>${fmt.num(d.stats.lossy_albums)}</b><span>Albums only in lossy quality</span></div>
      <div class="stat"><b>${fmt.num(d.stats.upgrades_done)}</b><span>Upgraded to lossless</span></div>
    </div>
    <div class="row wrap gap-lg" style="align-items:flex-start">
      <div class="card stack" style="flex:1;min-width:320px">
        <h2>What gets downloaded automatically</h2>
        ${sw('auto_request_likes', 'Liked songs', 'A song liked in Finamp, Plexamp, YouTube Music, Spotify, Tidal, Deezer… that isn’t in the library yet gets downloaded.')}
        ${sw('auto_request_followed', 'New tracks in followed playlists', 'Playlists set to “Follow source” pull new tracks every sync and download the missing ones.')}
        ${sw('auto_approve', 'Skip the approval queue for everyone', 'Off: users’ manual requests wait for an admin. Auto-downloads for enabled users are always approved.')}
        <label class="field"><span>Check for new likes / playlist tracks every</span>
          <div class="row"><input class="input" type="number" min="0" data-k="sync_interval_minutes" value="${esc(s.sync_interval_minutes)}" style="width:110px"><span class="muted">minutes (0 = never)</span></div></label>
      </div>
      <div class="card stack" style="flex:1;min-width:320px">
        <h2>Quality</h2>
        <label class="field"><span>When a song is part of an album</span>
          <select class="input" data-k="album_upgrade">
            <option value="album_ep">Download the whole album or EP it’s from (recommended)</option>
            <option value="album">Download the whole album (studio albums only)</option>
            <option value="off">Only download the song</option>
          </select>
          <small>Uses the official studio release from MusicBrainz – never compilations, live albums or soundtracks. If the album doesn’t contain that exact version (e.g. a remix), the song is fetched on its own.</small></label>
        ${sw('quality_first', 'Highest quality first', 'Try every lossless source (Qobuz/Tidal/Deezer, then FLAC on Soulseek – 24-bit preferred) before Lidarr, MP3s and YouTube Music.')}
        ${sw('upgrade_lossy', 'Keep upgrading lossy albums', 'Periodically look for lossless copies of albums you only have as MP3/YouTube rips. Better files replace the old ones; playlists follow automatically.')}
        <label class="field"><span>Retry each lossy album every</span>
          <div class="row"><input class="input" type="number" min="1" data-k="upgrade_lossy_days" value="${esc(s.upgrade_lossy_days)}" style="width:110px"><span class="muted">days</span>
          <button class="btn sm" id="upg">${icon.refresh}Look for upgrades now</button></div></label>
        <label class="field"><span>Source order</span><input class="input" data-k="source_order" value="${esc(s.source_order)}">
          <small>lidarr, streamrip, slskd, ytdlp (YouTube Music). With “highest quality first” lossless options always go first.</small></label>
      </div>
    </div>
    <div class="section-head"><h2>Users</h2><span class="small muted">Who auto-downloads are enabled for</span></div>
    <div class="card flat"><div class="table-wrap"><table class="table"><thead><tr><th>User</th><th>Followed playlists</th><th>Connected accounts</th><th>Auto-download</th></tr></thead><tbody>
      ${d.users.map(u => `<tr><td><b>${esc(u.username)}</b> ${u.role === 'admin' ? '<span class="badge">admin</span>' : ''}</td><td>${u.followed}</td><td>${u.connections}</td>
        <td><label class="switch"><input type="checkbox" data-user="${u.id}" ${u.auto_download ? 'checked' : ''}><i></i></label></td></tr>`).join('')}
    </tbody></table></div></div>`;
  view.querySelector('[data-k="album_upgrade"]').value = s.album_upgrade;
  const collect = () => {
    const settings = {};
    view.querySelectorAll('[data-k]').forEach(el => { settings[el.dataset.k] = el.type === 'checkbox' ? el.checked : el.value; });
    const users = {};
    view.querySelectorAll('[data-user]').forEach(el => { users[el.dataset.user] = el.checked; });
    return { settings, users };
  };
  view.querySelector('#save').onclick = async () => {
    try { d = await api.put('/admin/autodownload', collect()); toast('Auto-download rules saved', 'ok'); } catch (e) { toast(e.message, 'err'); }
  };
  view.querySelector('#run').onclick = async () => {
    await api.put('/admin/autodownload', collect());
    await api.post('/admin/autodownload/run-now');
    toast('Checking likes and followed playlists now…', 'ok');
  };
  view.querySelector('#upg').onclick = async () => {
    await api.put('/admin/autodownload', collect());
    const r = await api.post('/admin/autodownload/upgrade-now');
    toast(r.queued ? `Looking for lossless copies of ${r.queued} album(s)` : 'Nothing to upgrade right now', 'ok');
  };
}
