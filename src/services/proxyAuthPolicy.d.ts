declare function authenticatedProxyProtocols(
  url: string | URL,
  protocols: string | string[] | undefined,
  token: string,
  baseUrl: string,
): string[] | null;

declare const proxyAuthPolicy: {
  authenticatedProxyProtocols: typeof authenticatedProxyProtocols;
};

export default proxyAuthPolicy;
