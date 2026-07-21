const PROXY_PORTS = new Set(['3081', '3091']);

function authenticatedProxyProtocols(url, protocols, token, baseUrl) {
  const parsed = new URL(String(url), baseUrl);
  const isLocalProxy = (parsed.hostname === '127.0.0.1' || parsed.hostname === 'localhost')
    && PROXY_PORTS.has(parsed.port);
  if (!isLocalProxy) return null;

  const requested = protocols === undefined
    ? []
    : Array.isArray(protocols) ? [...protocols] : [protocols];
  if (!requested.includes('dirplayer-v1')) requested.push('dirplayer-v1');
  if (!requested.some((protocol) => protocol.startsWith('dirplayer-bearer.'))) {
    requested.push(`dirplayer-bearer.${token}`);
  }
  return requested;
}

module.exports = {
  authenticatedProxyProtocols,
};
