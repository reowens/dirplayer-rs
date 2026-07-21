const electron = require('electron');
const { app, BrowserWindow, dialog, ipcMain } = electron;
const path = require('path');
const { pathToFileURL } = require('url');
const fs = require('fs');
const isDev = require('electron-is-dev').default;
const { createMcpHttpServer, jsonRpcError } = require('./control-http.cjs');
const { isTrustedRendererUrl, secureWebPreferences } = require('./electron-policy.cjs');
const { folderGrantPrompt, isPathGranted } = require('./file-access-policy.cjs');
const {
  parseAllowedOrigins,
  resolveRunToken,
} = require('./control-security.cjs');

const MAX_LOCAL_FILE_BYTES = 256 * 1024 * 1024;
const MAX_MCP_RESPONSE_BYTES = 64 * 1024 * 1024;

let mainWindow;
let trustedRendererUrl;
let mcpService = null;
let mcpStatus = { state: 'off' };
let mcpRestartRequired = false;
let requestIdCounter = 0;
const pendingRendererRequests = new Map();
const grantedFileRoots = new Set();

function publishMcpStatus(status) {
  mcpStatus = status;
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('mcp:status', status);
  }
  return status;
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 900,
    height: 680,
    webPreferences: secureWebPreferences(path.join(__dirname, 'preload.cjs')),
  });
  trustedRendererUrl = isDev
    ? 'http://localhost:3000'
    : pathToFileURL(path.join(__dirname, '../build/index.html')).href;
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  mainWindow.webContents.on('will-navigate', (event, url) => {
    if (!isTrustedRendererUrl(url, trustedRendererUrl, isDev)) event.preventDefault();
  });
  mainWindow.loadURL(trustedRendererUrl);
  mainWindow.on('closed', () => {
    mainWindow = null;
  });
}

function isTrustedSender(event) {
  return Boolean(
    mainWindow
    && !mainWindow.isDestroyed()
    && event.sender === mainWindow.webContents
    && event.senderFrame === mainWindow.webContents.mainFrame
    && isTrustedRendererUrl(event.senderFrame.url, trustedRendererUrl, isDev)
  );
}

function requireTrustedSender(event) {
  if (!isTrustedSender(event)) throw new Error('Untrusted IPC sender');
}

async function readGrantedFile(filePath) {
  if (typeof filePath !== 'string' || filePath.length === 0 || filePath.length > 4096) {
    throw new Error('Invalid file path');
  }
  const realPath = await fs.promises.realpath(filePath);
  if (!isPathGranted(realPath, grantedFileRoots)) {
    throw new Error('File is outside a user-approved directory');
  }
  const stats = await fs.promises.stat(realPath);
  if (!stats.isFile()) throw new Error('Path is not a file');
  if (stats.size > MAX_LOCAL_FILE_BYTES) throw new Error('File exceeds local read limit');
  return fs.promises.readFile(realPath);
}

ipcMain.handle('dialog:openFile', async (event) => {
  requireTrustedSender(event);
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openFile'],
    filters: [
      { name: 'Director Movies', extensions: ['dir', 'dxr', 'dcr'] },
      { name: 'All Files', extensions: ['*'] },
    ],
  });
  if (result.canceled || result.filePaths.length === 0) return null;
  const selectedPath = await fs.promises.realpath(result.filePaths[0]);
  const folder = path.dirname(selectedPath);
  const confirmation = await dialog.showMessageBox(mainWindow, folderGrantPrompt(folder));
  if (confirmation.response !== 0) return null;
  grantedFileRoots.add(folder);
  return selectedPath;
});

ipcMain.handle('fs:readFile', async (event, filePath) => {
  requireTrustedSender(event);
  try {
    const data = await readGrantedFile(filePath);
    return { success: true, data };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : 'File read failed' };
  }
});

function rejectPendingRendererRequests(message) {
  for (const [requestId, pending] of pendingRendererRequests) {
    pending.signal.removeEventListener('abort', pending.onAbort);
    pending.reject(new Error(message));
    pendingRendererRequests.delete(requestId);
  }
}

function dispatchToRenderer(request, { signal }) {
  if (!mainWindow || mainWindow.isDestroyed()) {
    return Promise.resolve(jsonRpcError(request.id, -32603, 'VM not available'));
  }
  const requestId = `req_${++requestIdCounter}`;
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      if (!pendingRendererRequests.has(requestId)) return;
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('mcp:cancel', requestId);
      }
    };
    pendingRendererRequests.set(requestId, { resolve, reject, onAbort, signal });
    signal.addEventListener('abort', onAbort, { once: true });
    mainWindow.webContents.send('mcp:request', { requestId, request });
  });
}

async function startMcpServer(port) {
  if (mcpRestartRequired) {
    return publishMcpStatus({
      state: 'error',
      restartRequired: true,
      message: 'MCP timed out while VM work was still running. Restart DirPlayer before enabling MCP again.',
    });
  }
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return publishMcpStatus({ state: 'error', message: 'Invalid MCP port' });
  }
  if (mcpService) return mcpStatus;

  publishMcpStatus({ state: 'starting', host: '127.0.0.1', port });
  try {
    const token = resolveRunToken('DIRPLAYER_MCP_TOKEN');
    const allowedOrigins = parseAllowedOrigins(process.env.DIRPLAYER_MCP_ALLOWED_ORIGINS);
    const service = createMcpHttpServer({
      token,
      allowedOrigins,
      handleRequest: dispatchToRenderer,
      onDegraded: (message) => {
        if (mcpService !== service) return;
        mcpRestartRequired = true;
        publishMcpStatus({ state: 'error', restartRequired: true, message });
      },
    });
    mcpService = service;
    await service.listen(port, '127.0.0.1');
    service.server.on('error', (error) => {
      if (mcpService !== service) return;
      console.error('MCP server error:', error);
      mcpService = null;
      rejectPendingRendererRequests('MCP server failed');
      void service.close().finally(() => {
        publishMcpStatus({ state: 'error', message: error.message });
      });
    });
    console.log(`MCP server listening on http://127.0.0.1:${port}`);
    console.log(`MCP bearer token: ${token}`);
    return publishMcpStatus({
      state: 'listening',
      host: '127.0.0.1',
      port,
      token,
    });
  } catch (error) {
    if (mcpService) await mcpService.close().catch(() => {});
    mcpService = null;
    const message = error instanceof Error ? error.message : 'Failed to start MCP server';
    console.error('MCP server error:', error);
    return publishMcpStatus({ state: 'error', message });
  }
}

async function stopMcpServer() {
  const service = mcpService;
  mcpService = null;
  rejectPendingRendererRequests('MCP server stopping');
  if (service) await service.close();
  console.log('MCP server stopped');
  if (mcpRestartRequired) return mcpStatus;
  return publishMcpStatus({ state: 'off' });
}

ipcMain.handle('mcp:start-server', async (event, value) => {
  requireTrustedSender(event);
  return startMcpServer(value?.port);
});

ipcMain.handle('mcp:stop-server', async (event) => {
  requireTrustedSender(event);
  return stopMcpServer();
});

ipcMain.on('mcp:response', (event, value) => {
  if (!isTrustedSender(event)) return;
  const requestId = value?.requestId;
  const pending = typeof requestId === 'string' ? pendingRendererRequests.get(requestId) : null;
  if (!pending) return;
  pending.signal.removeEventListener('abort', pending.onAbort);
  let serialized;
  try {
    serialized = JSON.stringify(value.response);
  } catch (_error) {
    pending.reject(new Error('Renderer returned an invalid response'));
    pendingRendererRequests.delete(requestId);
    return;
  }
  if (Buffer.byteLength(serialized) > MAX_MCP_RESPONSE_BYTES) {
    pending.reject(new Error('Renderer response exceeds limit'));
  } else {
    pending.resolve(value.response);
  }
  pendingRendererRequests.delete(requestId);
});

app.on('ready', createWindow);

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});

app.on('activate', () => {
  if (mainWindow === null) createWindow();
});

app.on('before-quit', () => {
  void stopMcpServer();
});
