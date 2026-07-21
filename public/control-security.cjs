const crypto = require('crypto');

const TOKEN_BYTES = 32;
const TOKEN_PATTERN = /^[A-Za-z0-9_-]{43,128}$/;

function generateBearerToken() {
  return crypto.randomBytes(TOKEN_BYTES).toString('base64url');
}

function requireValidToken(token, name) {
  if (typeof token !== 'string' || !TOKEN_PATTERN.test(token)) {
    throw new Error(`${name} must contain 43 to 128 base64url characters`);
  }
  return token;
}

function resolveRunToken(environmentName) {
  const configured = process.env[environmentName];
  return configured
    ? requireValidToken(configured, environmentName)
    : generateBearerToken();
}

function tokensEqual(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string') return false;
  const leftBuffer = Buffer.from(left);
  const rightBuffer = Buffer.from(right);
  return leftBuffer.length === rightBuffer.length
    && crypto.timingSafeEqual(leftBuffer, rightBuffer);
}

function bearerTokenFromRequest(req) {
  const authorization = req.headers.authorization;
  if (typeof authorization !== 'string') return null;
  const match = /^Bearer ([A-Za-z0-9_-]+)$/.exec(authorization);
  return match ? match[1] : null;
}

function websocketTokenFromRequest(req) {
  const authorizationToken = bearerTokenFromRequest(req);
  if (authorizationToken) return authorizationToken;

  const protocols = typeof req.headers['sec-websocket-protocol'] === 'string'
    ? req.headers['sec-websocket-protocol'].split(',').map((value) => value.trim())
    : [];
  const protocol = protocols.find((value) => value.startsWith('dirplayer-bearer.'));
  return protocol ? protocol.slice('dirplayer-bearer.'.length) : null;
}

function parseAllowedOrigins(value) {
  if (!value) return [];
  return value.split(',').map((origin) => origin.trim()).filter(Boolean);
}

function isOriginAllowed(origin, allowedOrigins) {
  return origin === undefined || allowedOrigins.includes(origin);
}

function isAuthorized(req, expectedToken) {
  return tokensEqual(bearerTokenFromRequest(req), expectedToken);
}

module.exports = {
  bearerTokenFromRequest,
  generateBearerToken,
  isAuthorized,
  isOriginAllowed,
  parseAllowedOrigins,
  requireValidToken,
  resolveRunToken,
  tokensEqual,
  websocketTokenFromRequest,
};
