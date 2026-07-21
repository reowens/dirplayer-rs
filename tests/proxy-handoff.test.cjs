const assert = require('node:assert/strict');
const { test } = require('node:test');
const { createProxyStackEnvironment } = require('../scripts/start-proxy-stack.cjs');
const { authenticatedProxyProtocols } = require('../src/services/proxyAuthPolicy.cjs');

test('proxy stack generates and shares one run-scoped token', () => {
  const { token, environment } = createProxyStackEnvironment({ PATH: '/usr/bin' });
  assert.match(token, /^[A-Za-z0-9_-]{43}$/);
  assert.equal(environment.DIRPLAYER_PROXY_TOKEN, token);
  assert.equal(environment.REACT_APP_DIRPLAYER_PROXY_TOKEN, token);
  assert.equal(
    environment.DIRPLAYER_PROXY_ALLOWED_ORIGINS,
    'http://localhost:3000,http://127.0.0.1:3000',
  );
});

test('proxy stack shares an explicitly configured token unchanged', () => {
  const configured = 'c'.repeat(43);
  const { token, environment } = createProxyStackEnvironment({
    DIRPLAYER_PROXY_TOKEN: configured,
    DIRPLAYER_PROXY_ALLOWED_ORIGINS: 'http://localhost:4000',
  });
  assert.equal(token, configured);
  assert.equal(environment.DIRPLAYER_PROXY_TOKEN, configured);
  assert.equal(environment.REACT_APP_DIRPLAYER_PROXY_TOKEN, configured);
  assert.equal(environment.DIRPLAYER_PROXY_ALLOWED_ORIGINS, 'http://localhost:4000');
});

test('browser auth shim injects the shared token only for local proxy ports', () => {
  const original = ['existing'];
  assert.deepEqual(authenticatedProxyProtocols(
    'ws://127.0.0.1:3091',
    original,
    't'.repeat(43),
    'http://localhost:3000',
  ), ['existing', 'dirplayer-v1', `dirplayer-bearer.${'t'.repeat(43)}`]);
  assert.deepEqual(original, ['existing']);
  assert.equal(authenticatedProxyProtocols(
    'wss://example.com/socket',
    undefined,
    't'.repeat(43),
    'http://localhost:3000',
  ), null);
});
