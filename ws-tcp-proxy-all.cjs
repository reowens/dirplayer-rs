/**
 * Authenticated loopback WebSocket-to-TCP proxy for Director Multiuser Xtra.
 */

const http = require('http');
const net = require('net');
const WebSocket = require('ws');
const {
  isOriginAllowed,
  parseAllowedOrigins,
  resolveRunToken,
  tokensEqual,
  websocketTokenFromRequest,
} = require('./public/control-security.cjs');

const DEFAULT_LIMITS = Object.freeze({
  maxPayloadBytes: 1024 * 1024,
  maxConnections: 16,
  maxBufferedBytes: 2 * 1024 * 1024,
  connectTimeoutMs: 5_000,
  idleTimeoutMs: 10 * 60_000,
});

function rejectUpgrade(socket, statusCode, message) {
  if (socket.destroyed) return;
  const body = `${message}\n`;
  socket.end(
    `HTTP/1.1 ${statusCode} ${message}\r\n`
    + 'Connection: close\r\n'
    + 'Content-Type: text/plain; charset=utf-8\r\n'
    + `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n`
    + body,
  );
}

function createProxy(options) {
  const {
    wsPort,
    tcpHost,
    tcpPort,
    name,
    token,
    allowedOrigins = [],
    limits: limitOverrides = {},
  } = options;
  const limits = { ...DEFAULT_LIMITS, ...limitOverrides };
  const connections = new Set();
  let closing = false;

  const server = http.createServer((_req, res) => {
    res.writeHead(404, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' });
    res.end('WebSocket upgrade required\n');
  });
  server.requestTimeout = limits.connectTimeoutMs;
  server.headersTimeout = limits.connectTimeoutMs;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 32;

  const wss = new WebSocket.Server({
    noServer: true,
    clientTracking: true,
    maxPayload: limits.maxPayloadBytes,
    perMessageDeflate: false,
    handleProtocols: (protocols) => protocols.has('dirplayer-v1') ? 'dirplayer-v1' : false,
  });

  server.on('upgrade', (req, socket, head) => {
    const origin = typeof req.headers.origin === 'string' ? req.headers.origin : undefined;
    if (!isOriginAllowed(origin, allowedOrigins)) {
      rejectUpgrade(socket, 403, 'Forbidden');
      return;
    }
    if (!tokensEqual(websocketTokenFromRequest(req), token)) {
      rejectUpgrade(socket, 401, 'Unauthorized');
      return;
    }
    if (closing || wss.clients.size >= limits.maxConnections) {
      rejectUpgrade(socket, 503, 'Service Unavailable');
      return;
    }
    wss.handleUpgrade(req, socket, head, (ws) => {
      wss.emit('connection', ws, req);
    });
  });

  wss.on('connection', (ws, req) => {
    const clientIp = req.socket.remoteAddress;
    console.log(`[${name}] Authenticated WS connection from ${clientIp}`);
    const tcp = net.createConnection({ host: tcpHost, port: tcpPort });
    const connection = { ws, tcp, idleTimer: null, connectTimer: null, closed: false };
    connections.add(connection);

    const closeConnection = (code = 1011, reason = 'Proxy connection closed') => {
      if (connection.closed) return;
      connection.closed = true;
      clearTimeout(connection.connectTimer);
      clearTimeout(connection.idleTimer);
      connections.delete(connection);
      tcp.destroy();
      if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
        ws.close(code, reason.slice(0, 123));
      }
    };

    const resetIdleTimer = () => {
      clearTimeout(connection.idleTimer);
      connection.idleTimer = setTimeout(
        () => closeConnection(1001, 'Idle timeout'),
        limits.idleTimeoutMs,
      );
    };

    connection.connectTimer = setTimeout(
      () => closeConnection(1011, 'TCP connect timeout'),
      limits.connectTimeoutMs,
    );
    resetIdleTimer();

    tcp.on('connect', () => {
      clearTimeout(connection.connectTimer);
      console.log(`[${name}] Connected to TCP ${tcpHost}:${tcpPort}`);
    });

    tcp.on('data', (data) => {
      resetIdleTimer();
      if (ws.readyState !== WebSocket.OPEN) return;
      if (ws.bufferedAmount + data.length > limits.maxBufferedBytes) {
        closeConnection(1013, 'WebSocket backpressure limit');
        return;
      }
      ws.send(data, { binary: true }, (error) => {
        if (error) closeConnection(1011, 'WebSocket send failed');
      });
    });

    tcp.on('close', () => closeConnection(1000, 'TCP closed'));
    tcp.on('error', (error) => {
      console.error(`[${name}] TCP error: ${error.message}`);
      closeConnection(1011, 'TCP error');
    });

    ws.on('message', (data) => {
      resetIdleTimer();
      if (tcp.destroyed || !tcp.writable) return;
      if (tcp.writableLength + data.length > limits.maxBufferedBytes) {
        closeConnection(1013, 'TCP backpressure limit');
        return;
      }
      if (!tcp.write(data)) {
        req.socket.pause();
        tcp.once('drain', () => {
          if (!connection.closed) req.socket.resume();
        });
      }
    });

    ws.on('close', () => closeConnection(1000, 'WebSocket closed'));
    ws.on('error', (error) => {
      console.error(`[${name}] WS error: ${error.message}`);
      closeConnection(1011, 'WebSocket error');
    });
  });

  async function listen(host = '127.0.0.1') {
    if (host !== '127.0.0.1') throw new Error('Proxy must bind to 127.0.0.1');
    await new Promise((resolve, reject) => {
      const onError = (error) => {
        server.removeListener('listening', onListening);
        reject(error);
      };
      const onListening = () => {
        server.removeListener('error', onError);
        resolve();
      };
      server.once('error', onError);
      server.once('listening', onListening);
      server.listen(wsPort, host);
    });
    return server.address();
  }

  async function close() {
    if (closing) return;
    closing = true;
    for (const connection of [...connections]) {
      clearTimeout(connection.connectTimer);
      clearTimeout(connection.idleTimer);
      connection.closed = true;
      connection.tcp.destroy();
      connection.ws.terminate();
      connections.delete(connection);
    }
    await new Promise((resolve) => {
      wss.close(() => {
        if (!server.listening) {
          resolve();
          return;
        }
        server.close(resolve);
        if (typeof server.closeAllConnections === 'function') server.closeAllConnections();
      });
    });
  }

  return {
    server,
    wss,
    limits,
    listen,
    close,
    getConnectionCount: () => connections.size,
  };
}

async function main() {
  const token = resolveRunToken('DIRPLAYER_PROXY_TOKEN');
  const allowedOrigins = parseAllowedOrigins(process.env.DIRPLAYER_PROXY_ALLOWED_ORIGINS);
  const proxies = [
    createProxy({ wsPort: 3091, tcpHost: '127.0.0.1', tcpPort: 3090, name: 'Game', token, allowedOrigins }),
    createProxy({ wsPort: 3081, tcpHost: '127.0.0.1', tcpPort: 3080, name: 'Multiuser', token, allowedOrigins }),
  ];

  console.log('Authenticated WebSocket-to-TCP Proxy for Director');
  console.log(`Proxy bearer token: ${token}`);
  if (allowedOrigins.length > 0) console.log(`Allowed browser origins: ${allowedOrigins.join(', ')}`);
  await Promise.all(proxies.map(async (proxy) => {
    const address = await proxy.listen('127.0.0.1');
    console.log(`Listening on ws://127.0.0.1:${address.port}`);
  }));

  let stopping = false;
  const stop = async () => {
    if (stopping) return;
    stopping = true;
    await Promise.all(proxies.map((proxy) => proxy.close()));
  };
  process.once('SIGINT', () => void stop().finally(() => process.exit(0)));
  process.once('SIGTERM', () => void stop().finally(() => process.exit(0)));
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

module.exports = {
  DEFAULT_LIMITS,
  createProxy,
};
