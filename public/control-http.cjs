const http = require('http');
const {
  isAuthorized,
  isOriginAllowed,
} = require('./control-security.cjs');

const DEFAULT_LIMITS = Object.freeze({
  maxBodyBytes: 1024 * 1024,
  maxPendingRequests: 16,
  maxConcurrentWork: 1,
  bodyTimeoutMs: 5_000,
  operationTimeoutMs: 30_000,
});

function jsonRpcError(id, code, message) {
  return {
    jsonrpc: '2.0',
    id: id ?? null,
    error: { code, message },
  };
}

function createMcpHttpServer(options) {
  const {
    token,
    allowedOrigins = [],
    handleRequest,
    onDegraded = () => {},
    limits: limitOverrides = {},
  } = options;
  const limits = { ...DEFAULT_LIMITS, ...limitOverrides };
  const jobs = new Set();
  const queue = [];
  const sockets = new Set();
  let activeWork = 0;
  let closing = false;
  let degraded = false;
  let degradedReason = null;

  function writeJson(res, statusCode, body, extraHeaders = {}) {
    if (res.writableEnded || res.destroyed) return;
    res.writeHead(statusCode, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
      ...extraHeaders,
    });
    res.end(JSON.stringify(body));
  }

  function removeFromQueue(job) {
    const index = queue.indexOf(job);
    if (index >= 0) queue.splice(index, 1);
  }

  function finalize(job) {
    if (job.finalized) return;
    job.finalized = true;
    clearTimeout(job.bodyTimer);
    clearTimeout(job.deadlineTimer);
    job.req.removeListener('aborted', job.onClientGone);
    job.res.removeListener('close', job.onClientGone);
    removeFromQueue(job);
    jobs.delete(job);
    if (job.active) {
      job.active = false;
      activeWork -= 1;
    }
    drainQueue();
  }

  function fail(job, statusCode, code, message) {
    if (!job.responded) {
      job.responded = true;
      writeJson(job.res, statusCode, jsonRpcError(job.request?.id, code, message));
    }
    job.controller.abort();
    if (!job.active) finalize(job);
  }

  function onDeadline(job) {
    if (!job.active) {
      fail(job, 504, -32603, 'Request timeout while queued');
      return;
    }
    degrade(job);
  }

  function degrade(timedOutJob) {
    if (degraded || closing) return;
    degraded = true;
    degradedReason = 'VM operation timed out and may still be running. Restart DirPlayer before using MCP again.';
    fail(timedOutJob, 504, -32603, 'Request timeout; MCP restart required');
    for (const job of [...jobs]) {
      if (job === timedOutJob || job.finalized) continue;
      fail(job, 503, -32003, 'MCP degraded; restart DirPlayer before retrying');
    }
    onDegraded(degradedReason);
    if (server.listening) {
      server.close(() => {});
      if (typeof server.closeIdleConnections === 'function') server.closeIdleConnections();
    }
  }

  function startJob(job) {
    if (job.finalized || job.responded || closing || degraded) return;
    job.active = true;
    activeWork += 1;
    Promise.resolve()
      .then(() => handleRequest(job.request, { signal: job.controller.signal }))
      .then((response) => {
        if (!job.responded) {
          job.responded = true;
          writeJson(job.res, 200, response);
        }
      })
      .catch((error) => {
        if (!job.responded) {
          job.responded = true;
          writeJson(job.res, 500, jsonRpcError(
            job.request?.id,
            -32603,
            error instanceof Error ? error.message : 'Internal error',
          ));
        }
      })
      .finally(() => finalize(job));
  }

  function drainQueue() {
    if (closing || degraded) return;
    while (activeWork < limits.maxConcurrentWork && queue.length > 0) {
      const job = queue.shift();
      startJob(job);
    }
  }

  const server = http.createServer((req, res) => {
    const origin = typeof req.headers.origin === 'string' ? req.headers.origin : undefined;
    if (!isOriginAllowed(origin, allowedOrigins)) {
      writeJson(res, 403, jsonRpcError(null, -32001, 'Origin not allowed'));
      return;
    }
    if (origin) {
      res.setHeader('Access-Control-Allow-Origin', origin);
      res.setHeader('Vary', 'Origin');
    }

    if (req.method === 'OPTIONS') {
      res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
      res.setHeader('Access-Control-Allow-Headers', 'Authorization, Content-Type');
      res.setHeader('Access-Control-Max-Age', '600');
      res.writeHead(204);
      res.end();
      return;
    }

    if (req.method !== 'POST') {
      writeJson(res, 405, jsonRpcError(null, -32600, 'POST required'), { Allow: 'POST, OPTIONS' });
      return;
    }
    if (!isAuthorized(req, token)) {
      writeJson(res, 401, jsonRpcError(null, -32000, 'Authentication required'), {
        'WWW-Authenticate': 'Bearer',
      });
      return;
    }
    if (degraded) {
      writeJson(res, 503, jsonRpcError(null, -32003, 'MCP degraded; restart DirPlayer before retrying'));
      return;
    }
    if (closing) {
      writeJson(res, 503, jsonRpcError(null, -32603, 'Server stopping'));
      return;
    }
    if (jobs.size >= limits.maxPendingRequests) {
      writeJson(res, 503, jsonRpcError(null, -32002, 'Too many pending requests'), {
        'Retry-After': '1',
      });
      return;
    }

    const job = {
      req,
      res,
      request: null,
      controller: new AbortController(),
      bodyTimer: null,
      deadlineTimer: null,
      active: false,
      responded: false,
      finalized: false,
      onClientGone: null,
    };
    jobs.add(job);

    job.onClientGone = () => {
      if (job.finalized || job.res.writableEnded) return;
      job.responded = true;
      job.controller.abort();
      if (!job.active) finalize(job);
    };
    req.once('aborted', job.onClientGone);
    res.once('close', job.onClientGone);

    let bytesRead = 0;
    const chunks = [];
    job.bodyTimer = setTimeout(() => {
      fail(job, 408, -32700, 'Request body timeout');
      req.resume();
    }, limits.bodyTimeoutMs);

    req.on('data', (chunk) => {
      if (job.responded) return;
      bytesRead += chunk.length;
      if (bytesRead > limits.maxBodyBytes) {
        fail(job, 413, -32600, 'Request body too large');
        return;
      }
      chunks.push(chunk);
    });

    req.on('error', () => {
      if (!job.responded) fail(job, 400, -32700, 'Request read error');
    });

    req.on('end', () => {
      clearTimeout(job.bodyTimer);
      if (job.responded || job.finalized) return;
      try {
        job.request = JSON.parse(Buffer.concat(chunks, bytesRead).toString('utf8'));
      } catch (_error) {
        fail(job, 400, -32700, 'Parse error');
        return;
      }
      job.deadlineTimer = setTimeout(() => onDeadline(job), limits.operationTimeoutMs);
      queue.push(job);
      drainQueue();
    });
  });

  server.requestTimeout = limits.bodyTimeoutMs + limits.operationTimeoutMs;
  server.headersTimeout = limits.bodyTimeoutMs;
  server.keepAliveTimeout = 5_000;
  server.maxHeadersCount = 32;
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });

  async function listen(port, host = '127.0.0.1') {
    if (host !== '127.0.0.1') throw new Error('MCP server must bind to 127.0.0.1');
    if (degraded) throw new Error('MCP is degraded; restart DirPlayer before listening again');
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
      server.listen(port, host);
    });
    return server.address();
  }

  async function close() {
    if (closing) return;
    closing = true;
    for (const job of [...jobs]) {
      if (!job.responded) {
        job.responded = true;
        writeJson(job.res, 503, jsonRpcError(job.request?.id, -32603, 'Server stopping'));
      }
      job.controller.abort();
      finalize(job);
    }

    if (!server.listening) {
      for (const socket of sockets) socket.destroy();
      return;
    }
    await new Promise((resolve) => {
      server.close(resolve);
      if (typeof server.closeAllConnections === 'function') server.closeAllConnections();
      for (const socket of sockets) socket.destroy();
    });
  }

  return {
    server,
    limits,
    listen,
    close,
    getPendingCount: () => jobs.size,
    getActiveCount: () => activeWork,
    getState: () => degraded
      ? { state: 'degraded', reason: degradedReason }
      : { state: closing ? 'closed' : server.listening ? 'listening' : 'idle' },
  };
}

module.exports = {
  DEFAULT_LIMITS,
  createMcpHttpServer,
  jsonRpcError,
};
