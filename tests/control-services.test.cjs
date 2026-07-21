const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const { afterEach, describe, test } = require('node:test');
const WebSocket = require('ws');
const { createMcpHttpServer } = require('../public/control-http.cjs');
const { generateBearerToken, requireValidToken } = require('../public/control-security.cjs');
const { isTrustedRendererUrl, secureWebPreferences } = require('../public/electron-policy.cjs');
const { folderGrantPrompt, isPathGranted } = require('../public/file-access-policy.cjs');
const { authenticatedProxyProtocols } = require('../src/services/proxyAuthPolicy.cjs');
const { createProxy } = require('../ws-tcp-proxy-all.cjs');

const resources = [];

afterEach(async () => {
  await Promise.all(resources.splice(0).reverse().map((close) => close().catch(() => {})));
});

function request({ port, token, origin, body, method = 'POST' }) {
  return new Promise((resolve, reject) => {
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers.Authorization = `Bearer ${token}`;
    if (origin) headers.Origin = origin;
    const req = http.request({ host: '127.0.0.1', port, method, headers }, (res) => {
      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve({
        status: res.statusCode,
        body: Buffer.concat(chunks).toString('utf8'),
      }));
    });
    req.on('error', reject);
    if (body !== undefined) req.end(typeof body === 'string' ? body : JSON.stringify(body));
    else req.end();
  });
}

async function startMcp(overrides = {}) {
  const token = overrides.token || generateBearerToken();
  const service = createMcpHttpServer({
    token,
    allowedOrigins: overrides.allowedOrigins || [],
    handleRequest: overrides.handleRequest || (async (value) => ({ ok: value.id })),
    onDegraded: overrides.onDegraded,
    limits: overrides.limits,
  });
  const address = await service.listen(0);
  resources.push(() => service.close());
  return { token, service, port: address.port };
}

function websocketFailure(url, options) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, options);
    ws.once('unexpected-response', (_req, response) => {
      response.resume();
      resolve(response.statusCode);
    });
    ws.once('open', () => {
      ws.close();
      reject(new Error('WebSocket unexpectedly opened'));
    });
    ws.once('error', () => {});
  });
}

function openWebSocket(url, protocols, options) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, protocols, options);
    ws.once('open', () => resolve(ws));
    ws.once('error', reject);
  });
}

async function startTcpServer() {
  const server = net.createServer((socket) => socket.pipe(socket));
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  resources.push(() => new Promise((resolve) => server.close(resolve)));
  return server.address().port;
}

async function startProxy(overrides = {}) {
  const token = overrides.token || generateBearerToken();
  const tcpPort = overrides.tcpPort || await startTcpServer();
  const proxy = createProxy({
    wsPort: 0,
    tcpHost: '127.0.0.1',
    tcpPort,
    name: 'Test',
    token,
    allowedOrigins: overrides.allowedOrigins || [],
    limits: overrides.limits,
  });
  const address = await proxy.listen();
  resources.push(() => proxy.close());
  return { token, proxy, port: address.port };
}

describe('MCP HTTP transport', () => {
  test('requires the correct bearer token and enforces Origin', async () => {
    const { token, port } = await startMcp({ allowedOrigins: ['http://localhost:3000'] });
    const payload = { jsonrpc: '2.0', id: 7, method: 'tools/list' };

    assert.equal((await request({ port, body: payload })).status, 401);
    assert.equal((await request({ port, token: generateBearerToken(), body: payload })).status, 401);
    assert.equal((await request({ port, token, origin: 'https://evil.example', body: payload })).status, 403);
    const response = await request({ port, token, origin: 'http://localhost:3000', body: payload });
    assert.equal(response.status, 200);
    assert.deepEqual(JSON.parse(response.body), { ok: 7 });
  });

  test('rejects oversized request bodies', async () => {
    const { token, port } = await startMcp({ limits: { maxBodyBytes: 32 } });
    const response = await request({ port, token, body: JSON.stringify({ value: 'x'.repeat(100) }) });
    assert.equal(response.status, 413);
  });

  test('bounds queued and concurrent work', async () => {
    const resolvers = [];
    const { token, port, service } = await startMcp({
      limits: { maxConcurrentWork: 1, maxPendingRequests: 2 },
      handleRequest: () => new Promise((resolve) => resolvers.push(resolve)),
    });
    const first = request({ port, token, body: { id: 1 } });
    while (resolvers.length < 1) await new Promise((resolve) => setTimeout(resolve, 5));
    const second = request({ port, token, body: { id: 2 } });
    while (service.getPendingCount() < 2) await new Promise((resolve) => setTimeout(resolve, 5));
    const third = await request({ port, token, body: { id: 3 } });
    assert.equal(third.status, 503);
    assert.equal(service.getActiveCount(), 1);

    resolvers.shift()({ id: 1 });
    assert.equal((await first).status, 200);
    while (resolvers.length < 1) await new Promise((resolve) => setTimeout(resolve, 5));
    resolvers.shift()({ id: 2 });
    assert.equal((await second).status, 200);
  });

  test('degrades on hung work, fails the queue, and requires restart', async () => {
    let resolveWork;
    const degraded = [];
    const { token, port, service } = await startMcp({
      limits: { operationTimeoutMs: 30 },
      handleRequest: () => new Promise((resolve) => { resolveWork = resolve; }),
      onDegraded: (message) => degraded.push(message),
    });
    const active = request({ port, token, body: { id: 4 } });
    while (service.getActiveCount() !== 1) await new Promise((resolve) => setTimeout(resolve, 5));
    const queued = request({ port, token, body: { id: 5 } });
    while (service.getPendingCount() !== 2) await new Promise((resolve) => setTimeout(resolve, 5));

    const [activeResponse, queuedResponse] = await Promise.all([active, queued]);
    assert.equal(activeResponse.status, 504);
    assert.match(activeResponse.body, /restart required/i);
    assert.equal(queuedResponse.status, 503);
    assert.match(queuedResponse.body, /degraded.*restart/i);
    assert.equal(degraded.length, 1);
    assert.match(degraded[0], /may still be running.*restart/i);
    assert.equal(service.getState().state, 'degraded');
    assert.equal(service.server.listening, false);
    assert.equal(service.getActiveCount(), 1);
    await assert.rejects(service.listen(0), /degraded.*restart/i);

    resolveWork({ id: 4 });
    while (service.getActiveCount() !== 0) await new Promise((resolve) => setTimeout(resolve, 5));
    assert.equal(service.getPendingCount(), 0);
  });

  test('aborts and removes pending work on stop', async () => {
    let aborted = false;
    const { token, port, service } = await startMcp({
      handleRequest: (_request, { signal }) => new Promise(() => {
        signal.addEventListener('abort', () => { aborted = true; });
      }),
    });
    const pending = request({ port, token, body: { id: 5 } });
    while (service.getActiveCount() !== 1) await new Promise((resolve) => setTimeout(resolve, 5));
    await service.close();
    const response = await pending;
    assert.equal(response.status, 503);
    assert.equal(aborted, true);
    assert.equal(service.getPendingCount(), 0);
  });

  test('binds only to IPv4 loopback', async () => {
    const { service } = await startMcp();
    assert.equal(service.server.address().address, '127.0.0.1');
  });
});

describe('WebSocket proxy transport', () => {
  test('requires auth and enforces Origin', async () => {
    const { token, port } = await startProxy({ allowedOrigins: ['http://localhost:3000'] });
    const url = `ws://127.0.0.1:${port}`;
    assert.equal(await websocketFailure(url, { origin: 'http://localhost:3000' }), 401);
    assert.equal(await websocketFailure(url, {
      origin: 'http://localhost:3000',
      headers: { Authorization: `Bearer ${generateBearerToken()}` },
    }), 401);
    assert.equal(await websocketFailure(url, {
      origin: 'https://evil.example',
      headers: { Authorization: `Bearer ${token}` },
    }), 403);

    const ws = await openWebSocket(url, undefined, {
      origin: 'http://localhost:3000',
      headers: { Authorization: `Bearer ${token}` },
    });
    ws.close();
  });

  test('supports browser-compatible subprotocol auth', async () => {
    const { token, port } = await startProxy({ allowedOrigins: ['http://localhost:3000'] });
    const protocols = authenticatedProxyProtocols(
      'ws://127.0.0.1:3091',
      undefined,
      token,
      'http://localhost:3000',
    );
    const ws = await openWebSocket(
      `ws://127.0.0.1:${port}`,
      protocols,
      { origin: 'http://localhost:3000' },
    );
    assert.equal(ws.protocol, 'dirplayer-v1');
    ws.close();
  });

  test('bounds connections and payload bytes', async () => {
    const { token, port } = await startProxy({
      limits: { maxConnections: 1, maxPayloadBytes: 16 },
    });
    const url = `ws://127.0.0.1:${port}`;
    const options = { headers: { Authorization: `Bearer ${token}` } };
    const first = await openWebSocket(url, undefined, options);
    assert.equal(await websocketFailure(url, options), 503);

    const closed = new Promise((resolve) => first.once('close', (code) => resolve(code)));
    first.send(Buffer.alloc(32));
    assert.equal(await closed, 1009);
  });

  test('enforces idle timeout and cleans both sides', async () => {
    const { token, port, proxy } = await startProxy({ limits: { idleTimeoutMs: 30 } });
    const ws = await openWebSocket(`ws://127.0.0.1:${port}`, undefined, {
      headers: { Authorization: `Bearer ${token}` },
    });
    const code = await new Promise((resolve) => ws.once('close', resolve));
    assert.equal(code, 1001);
    assert.equal(proxy.getConnectionCount(), 0);
  });

  test('terminates active connections on stop', async () => {
    const { token, port, proxy } = await startProxy();
    const ws = await openWebSocket(`ws://127.0.0.1:${port}`, undefined, {
      headers: { Authorization: `Bearer ${token}` },
    });
    const closed = new Promise((resolve) => ws.once('close', resolve));
    await proxy.close();
    await closed;
    assert.equal(proxy.getConnectionCount(), 0);
  });

  test('binds only to IPv4 loopback', async () => {
    const { proxy } = await startProxy();
    assert.equal(proxy.server.address().address, '127.0.0.1');
  });
});

describe('Electron security policy', () => {
  test('disables renderer Node access and enables isolation and sandboxing', () => {
    assert.deepEqual(secureWebPreferences('/tmp/preload.cjs'), {
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      preload: '/tmp/preload.cjs',
    });
  });

  test('accepts only the configured renderer location', () => {
    assert.equal(isTrustedRendererUrl(
      'http://localhost:3000/path',
      'http://localhost:3000',
      true,
    ), true);
    assert.equal(isTrustedRendererUrl(
      'http://127.0.0.1:3000',
      'http://localhost:3000',
      true,
    ), false);
    assert.equal(isTrustedRendererUrl(
      'file:///app/build/index.html',
      'file:///app/build/index.html',
      false,
    ), true);
    assert.equal(isTrustedRendererUrl(
      'file:///tmp/other.html',
      'file:///app/build/index.html',
      false,
    ), false);
  });

  test('requires explicit parent-folder approval and enforces grant boundaries', () => {
    const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'dirplayer-file-grant-'));
    try {
      const root = path.join(temp, 'movie');
      const outsideRoot = path.join(temp, 'movie-other');
      fs.mkdirSync(root);
      fs.mkdirSync(outsideRoot);
      const selected = path.join(root, 'selected.dcr');
      const sibling = path.join(root, 'sibling.cct');
      const outside = path.join(outsideRoot, 'outside.cct');
      fs.writeFileSync(selected, 'selected');
      fs.writeFileSync(sibling, 'sibling');
      fs.writeFileSync(outside, 'outside');

      const prompt = folderGrantPrompt(root);
      assert.deepEqual(prompt.buttons, ['Allow Folder', 'Cancel']);
      assert.equal(prompt.defaultId, 1);
      assert.equal(prompt.cancelId, 1);
      assert.match(prompt.detail, /current app session/);
      assert.match(prompt.detail, new RegExp(root.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));

      const grants = new Set([fs.realpathSync(root)]);
      assert.equal(isPathGranted(fs.realpathSync(selected), grants), true);
      assert.equal(isPathGranted(fs.realpathSync(sibling), grants), true);
      assert.equal(isPathGranted(fs.realpathSync(outside), grants), false);
      const escapeLink = path.join(root, 'escape.cct');
      try {
        fs.symlinkSync(outside, escapeLink);
        assert.equal(isPathGranted(fs.realpathSync(escapeLink), grants), false);
      } catch (error) {
        if (error.code !== 'EPERM' && error.code !== 'EACCES') throw error;
      }
    } finally {
      fs.rmSync(temp, { recursive: true, force: true });
    }
  });
});

describe('token validation', () => {
  test('generated tokens are 256-bit random base64url values', () => {
    const first = generateBearerToken();
    const second = generateBearerToken();
    assert.match(first, /^[A-Za-z0-9_-]{43}$/);
    assert.notEqual(first, second);
  });

  test('configured tokens are validated by representation without claiming entropy', () => {
    assert.equal(requireValidToken('a'.repeat(43), 'TOKEN'), 'a'.repeat(43));
    assert.throws(
      () => requireValidToken('short', 'TOKEN'),
      /43 to 128 base64url characters/,
    );
  });
});
