function secureWebPreferences(preload) {
  return {
    nodeIntegration: false,
    contextIsolation: true,
    sandbox: true,
    preload,
  };
}

function isTrustedRendererUrl(value, expectedValue, development) {
  try {
    const candidate = new URL(value);
    const expected = new URL(expectedValue);
    if (development) return candidate.origin === expected.origin;
    return candidate.protocol === 'file:' && candidate.href === expected.href;
  } catch (_error) {
    return false;
  }
}

module.exports = {
  isTrustedRendererUrl,
  secureWebPreferences,
};
