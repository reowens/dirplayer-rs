/**
 * Utility functions for Electron environment detection and IPC communication
 */

/**
 * Check if the app is running in Electron
 */
export function isElectron(): boolean {
  if (typeof window !== 'undefined' && window.dirplayerElectron) {
    return true;
  }

  // Main process
  if (typeof process !== 'undefined' && typeof process.versions === 'object' && !!(process.versions as any).electron) {
    return true;
  }

  // Detect the user agent when the `nodeIntegration` option is set to false
  if (typeof navigator === 'object' && typeof navigator.userAgent === 'string' && navigator.userAgent.indexOf('Electron') >= 0) {
    return true;
  }

  return false;
}

/**
 * Open file dialog in Electron
 * Returns the selected file path or null if cancelled
 */
export async function openFileDialog(): Promise<string | null> {
  if (!window.dirplayerElectron) {
    throw new Error('openFileDialog can only be called in Electron environment');
  }
  return window.dirplayerElectron.openFileDialog();
}

/**
 * Read a file from the local filesystem in Electron
 * Returns the file data as a Uint8Array
 */
export async function readLocalFile(filePath: string): Promise<Uint8Array> {
  if (!window.dirplayerElectron) {
    throw new Error('readLocalFile can only be called in Electron environment');
  }
  const result = await window.dirplayerElectron.readLocalFile(filePath);

  if (result.success && result.data) {
    return new Uint8Array(result.data);
  } else {
    throw new Error(`Failed to read file: ${result.error || 'Unknown error'}`);
  }
}

export function getElectronPlatform(): string | null {
  return window.dirplayerElectron?.platform || null;
}
