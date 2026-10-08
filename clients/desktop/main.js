// Encore desktop: the Encore web app in its own window – media keys, a tray with play/pause/next,
// remembers your server, opens outside links in the browser.
const { app, BrowserWindow, Menu, Tray, shell, ipcMain, nativeImage, dialog } = require('electron');
const fs = require('fs');
const path = require('path');

const cfgFile = () => path.join(app.getPath('userData'), 'config.json');
const readCfg = () => { try { return JSON.parse(fs.readFileSync(cfgFile(), 'utf8')); } catch { return {}; } };
const writeCfg = (c) => fs.writeFileSync(cfgFile(), JSON.stringify(c, null, 2));
let win, tray;

function serverUrl() { return (readCfg().server || '').replace(/\/+$/, ''); }

function setup() { win.loadFile(path.join(__dirname, 'setup.html')); }

function open() {
  const s = serverUrl();
  if (!s) return setup();
  win.loadURL(s).catch(() => setup());
}

function control(action) {
  if (win) win.webContents.executeJavaScript(`window.encorePlayer && window.encorePlayer.${action}()`).catch(() => {});
}

function createWindow() {
  const st = readCfg().bounds || { width: 1280, height: 860 };
  win = new BrowserWindow({
    ...st, minWidth: 380, minHeight: 500, backgroundColor: '#0b0b12', title: 'Encore', autoHideMenuBar: true,
    icon: path.join(__dirname, 'build', 'icon.png'),
    webPreferences: { preload: path.join(__dirname, 'preload.js'), contextIsolation: true, sandbox: true, backgroundThrottling: false },
  });
  win.on('close', () => { const c = readCfg(); c.bounds = win.getBounds(); writeCfg(c); });
  // outside links → default browser; the Encore server (and its /svc pages) stay in the app
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (serverUrl() && url.startsWith(serverUrl())) return { action: 'allow' };
    shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (e, url) => {
    if (url.startsWith('file:') || (serverUrl() && url.startsWith(serverUrl()))) return;
    e.preventDefault();
    shell.openExternal(url);
  });
  win.webContents.on('did-fail-load', (_e, code, desc, url, isMain) => {
    if (isMain && serverUrl() && url.startsWith(serverUrl())) {
      dialog.showMessageBox(win, { type: 'warning', message: `Can't reach ${serverUrl()}`, detail: desc, buttons: ['Retry', 'Change server'] })
        .then(r => (r.response === 0 ? open() : setup()));
    }
  });
  open();
}

function buildMenu() {
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    ...(process.platform === 'darwin' ? [{ role: 'appMenu' }] : []),
    { label: 'Encore', submenu: [
      { label: 'Change server…', click: setup },
      { label: 'Reload', accelerator: 'CmdOrCtrl+R', click: () => win.reload() },
      { type: 'separator' },
      { label: 'Play / Pause', accelerator: 'MediaPlayPause', click: () => control('toggle') },
      { label: 'Next', accelerator: 'MediaNextTrack', click: () => control('next') },
      { label: 'Previous', accelerator: 'MediaPreviousTrack', click: () => control('prev') },
      { type: 'separator' },
      { role: 'toggleDevTools' }, { role: 'quit' },
    ] },
    { role: 'editMenu' }, { role: 'viewMenu' }, { role: 'windowMenu' },
  ]));
}

function buildTray(np) {
  if (!tray) {
    tray = new Tray(nativeImage.createFromPath(path.join(__dirname, 'build', 'icon.png')).resize({ width: 18, height: 18 }));
    tray.on('click', () => { win.show(); win.focus(); });
  }
  tray.setToolTip(np && !np.stopped ? `${np.title} – ${np.artist}` : 'Encore');
  tray.setContextMenu(Menu.buildFromTemplate([
    ...(np && !np.stopped ? [{ label: `${np.title} – ${np.artist}`.slice(0, 60), enabled: false }, { type: 'separator' }] : []),
    { label: np && np.playing ? 'Pause' : 'Play', click: () => control('toggle') },
    { label: 'Next', click: () => control('next') },
    { label: 'Previous', click: () => control('prev') },
    { type: 'separator' },
    { label: 'Show Encore', click: () => { win.show(); win.focus(); } },
    { label: 'Quit', click: () => app.quit() },
  ]));
}

ipcMain.handle('encore:set-server', async (_e, raw) => {
  let url = String(raw || '').trim();
  if (!/^https?:\/\//i.test(url)) url = 'http://' + url;
  url = url.replace(/\/+$/, '');
  try {
    const r = await fetch(url + '/api/health', { signal: AbortSignal.timeout(6000) });
    if (!r.ok) throw new Error(`answered ${r.status}`);
    const j = await r.json();
    if (!j.ok) throw new Error('not an Encore server');
  } catch (e) {
    return { ok: false, error: `No Encore server at ${url} (${e.message})` };
  }
  writeCfg({ ...readCfg(), server: url });
  open();
  return { ok: true };
});
ipcMain.handle('encore:get-server', () => serverUrl());
ipcMain.on('encore:change-server', () => setup());
ipcMain.on('encore:now-playing', (_e, json) => { try { buildTray(JSON.parse(json)); } catch { /* ignore */ } });

if (!app.requestSingleInstanceLock()) app.quit();
app.on('second-instance', () => { if (win) { if (win.isMinimized()) win.restore(); win.focus(); } });
app.whenReady().then(() => { buildMenu(); createWindow(); buildTray(null); });
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });
app.on('activate', () => { if (BrowserWindow.getAllWindows().length === 0) createWindow(); });
