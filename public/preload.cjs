const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('dirplayerElectron', Object.freeze({
  platform: process.platform,
  openFileDialog: () => ipcRenderer.invoke('dialog:openFile'),
  readLocalFile: (filePath) => ipcRenderer.invoke('fs:readFile', filePath),
  mcp: Object.freeze({
    start: (port) => ipcRenderer.invoke('mcp:start-server', { port }),
    stop: () => ipcRenderer.invoke('mcp:stop-server'),
    onRequest: (callback) => {
      const listener = (_event, data) => callback(data);
      ipcRenderer.on('mcp:request', listener);
      return () => ipcRenderer.removeListener('mcp:request', listener);
    },
    onCancel: (callback) => {
      const listener = (_event, requestId) => callback(requestId);
      ipcRenderer.on('mcp:cancel', listener);
      return () => ipcRenderer.removeListener('mcp:cancel', listener);
    },
    onStatus: (callback) => {
      const listener = (_event, status) => callback(status);
      ipcRenderer.on('mcp:status', listener);
      return () => ipcRenderer.removeListener('mcp:status', listener);
    },
    sendResponse: (requestId, response) => {
      ipcRenderer.send('mcp:response', { requestId, response });
    },
  }),
}));
