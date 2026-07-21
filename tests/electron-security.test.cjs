const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { _electron: electron } = require('playwright');

function mcpRequest(port, token, body = '{') {
  return new Promise((resolve, reject) => {
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers.Authorization = `Bearer ${token}`;
    const req = http.request({ host: '127.0.0.1', port, method: 'POST', headers }, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve({
        status: res.statusCode,
        body: Buffer.concat(chunks).toString('utf8'),
      }));
    });
    req.on('error', reject);
    req.end(body);
  });
}

test('renderer receives only the isolated preload API', async () => {
  const userDataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'dirplayer-electron-test-'));
  let electronApp;
  try {
    electronApp = await electron.launch({
      args: ['.', `--user-data-dir=${userDataDir}`],
      cwd: path.resolve(__dirname, '..'),
      env: { ...process.env, ELECTRON_IS_DEV: '0' },
    });
    const window = await electronApp.firstWindow();
    const securityState = await window.evaluate(() => ({
      requireType: typeof (globalThis).require,
      processType: typeof (globalThis).process,
      apiKeys: Object.keys(window.dirplayerElectron || {}).sort(),
      mcpKeys: Object.keys(window.dirplayerElectron?.mcp || {}).sort(),
      platformType: typeof window.dirplayerElectron?.platform,
    }));

    assert.equal(securityState.requireType, 'undefined');
    assert.equal(securityState.processType, 'undefined');
    assert.deepEqual(securityState.apiKeys, ['mcp', 'openFileDialog', 'platform', 'readLocalFile']);
    assert.deepEqual(securityState.mcpKeys, ['onCancel', 'onRequest', 'onStatus', 'sendResponse', 'start', 'stop']);
    assert.equal(securityState.platformType, 'string');

    assert.equal(await window.evaluate(() => localStorage.getItem('mcp:enabled')), null);
    const mcpStatus = await window.evaluate(() => new Promise((resolve, reject) => {
      const timeout = window.setTimeout(() => reject(new Error('MCP did not start')), 10_000);
      window.dirplayerElectron.mcp.onRequest(({ requestId, request }) => {
        window.dirplayerElectron.mcp.sendResponse(requestId, {
          jsonrpc: '2.0',
          id: request.id,
          result: { bridged: true },
        });
      });
      const remove = window.dirplayerElectron.mcp.onStatus((status) => {
        if (status.state !== 'listening') return;
        window.clearTimeout(timeout);
        remove();
        resolve(status);
      });
      window.dirplayerElectron.mcp.start(19847).catch(reject);
    }));
    assert.equal(mcpStatus.host, '127.0.0.1');
    assert.match(mcpStatus.token, /^[A-Za-z0-9_-]{43}$/);
    assert.equal((await mcpRequest(mcpStatus.port)).status, 401);
    const authenticated = await mcpRequest(mcpStatus.port, mcpStatus.token);
    assert.equal(authenticated.status, 400);
    const bridged = await mcpRequest(
      mcpStatus.port,
      mcpStatus.token,
      JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/list' }),
    );
    assert.equal(bridged.status, 200);
    assert.deepEqual(JSON.parse(bridged.body).result, { bridged: true });
    const stopped = await window.evaluate(() => window.dirplayerElectron.mcp.stop());
    assert.equal(stopped.state, 'off');
  } finally {
    if (electronApp) await electronApp.close();
    fs.rmSync(userDataDir, { recursive: true, force: true });
  }
});
