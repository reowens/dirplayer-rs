const { spawn } = require('child_process');
const {
  generateBearerToken,
  requireValidToken,
} = require('../public/control-security.cjs');

function createProxyStackEnvironment(baseEnvironment = process.env) {
  const configured = baseEnvironment.DIRPLAYER_PROXY_TOKEN;
  const token = configured
    ? requireValidToken(configured, 'DIRPLAYER_PROXY_TOKEN')
    : generateBearerToken();
  return {
    token,
    environment: {
      ...baseEnvironment,
      DIRPLAYER_PROXY_TOKEN: token,
      REACT_APP_DIRPLAYER_PROXY_TOKEN: token,
      DIRPLAYER_PROXY_ALLOWED_ORIGINS: baseEnvironment.DIRPLAYER_PROXY_ALLOWED_ORIGINS
        || 'http://localhost:3000,http://127.0.0.1:3000',
    },
  };
}

function main() {
  const { token, environment } = createProxyStackEnvironment();
  const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';
  console.log(`Proxy bearer token: ${token}`);
  const children = [
    spawn(npm, ['run', 'start'], { stdio: 'inherit', env: environment }),
    spawn(process.execPath, ['ws-tcp-proxy-all.cjs'], { stdio: 'inherit', env: environment }),
  ];

  let stopping = false;
  function stop(signal = 'SIGTERM') {
    if (stopping) return;
    stopping = true;
    for (const child of children) {
      if (!child.killed) child.kill(signal);
    }
  }

  for (const child of children) {
    child.on('exit', (code) => {
      if (!stopping) {
        process.exitCode = code || 0;
        stop();
      }
    });
  }
  process.once('SIGINT', () => stop('SIGINT'));
  process.once('SIGTERM', () => stop('SIGTERM'));
}

if (require.main === module) main();

module.exports = {
  createProxyStackEnvironment,
};
