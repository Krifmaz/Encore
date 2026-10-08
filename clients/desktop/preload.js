// The bridge the Encore web app looks for (window.EncoreNative): now-playing info for the tray, "Change server…",
// and the first-run setup page.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('EncoreNative', {
  nowPlaying: (json) => ipcRenderer.send('encore:now-playing', String(json)),
  changeServer: () => ipcRenderer.send('encore:change-server'),
  setServer: (url) => ipcRenderer.invoke('encore:set-server', url),
  getServer: () => ipcRenderer.invoke('encore:get-server'),
});
