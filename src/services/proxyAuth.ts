import proxyAuthPolicy from './proxyAuthPolicy.cjs';

const { authenticatedProxyProtocols } = proxyAuthPolicy;

export function installProxyWebSocketAuth(token: string | undefined): void {
  if (!token || typeof window.WebSocket !== 'function') return;
  const NativeWebSocket = window.WebSocket;

  function AuthenticatedWebSocket(url: string | URL, protocols?: string | string[]): WebSocket {
    const authenticated = authenticatedProxyProtocols(
      url,
      protocols,
      token,
      window.location.href,
    );
    if (!authenticated) {
      return protocols === undefined
        ? new NativeWebSocket(url)
        : new NativeWebSocket(url, protocols);
    }
    return new NativeWebSocket(url, authenticated);
  }

  AuthenticatedWebSocket.prototype = NativeWebSocket.prototype;
  Object.defineProperties(AuthenticatedWebSocket, {
    CONNECTING: { value: NativeWebSocket.CONNECTING },
    OPEN: { value: NativeWebSocket.OPEN },
    CLOSING: { value: NativeWebSocket.CLOSING },
    CLOSED: { value: NativeWebSocket.CLOSED },
  });
  window.WebSocket = AuthenticatedWebSocket as unknown as typeof WebSocket;
}
