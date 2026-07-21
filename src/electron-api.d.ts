export {};

declare global {
  interface McpTransportStatus {
    state: 'off' | 'starting' | 'listening' | 'error';
    host?: string;
    port?: number;
    token?: string;
    message?: string;
    restartRequired?: boolean;
  }

  interface Window {
    dirplayerProxyToken?: string;
    dirplayerElectron?: {
      platform: string;
      openFileDialog: () => Promise<string | null>;
      readLocalFile: (filePath: string) => Promise<{
        success: boolean;
        data?: Uint8Array | number[];
        error?: string;
      }>;
      mcp: {
        start: (port: number) => Promise<McpTransportStatus>;
        stop: () => Promise<McpTransportStatus>;
        onRequest: (callback: (data: { requestId: string; request: unknown }) => void) => () => void;
        onCancel: (callback: (requestId: string) => void) => () => void;
        onStatus: (callback: (status: McpTransportStatus) => void) => () => void;
        sendResponse: (requestId: string, response: unknown) => void;
      };
    };
  }
}
