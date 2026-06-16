type LocalNetworkRequestInit = RequestInit & {
  targetAddressSpace?: "local" | "loopback";
};

function requestUrl(input: RequestInfo | URL): string {
  if (typeof input === "string") return input;
  if (input instanceof URL) return input.href;
  return input.url;
}

function isLoopbackHost(hostname: string): boolean {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (host === "localhost" || host === "::1") return true;
  return /^127(?:\.\d{1,3}){3}$/.test(host);
}

export function targetAddressSpaceFor(input: RequestInfo | URL): "local" | "loopback" {
  try {
    const base =
      typeof window === "undefined" ? "https://gitim.io/" : window.location.href;
    const url = new URL(requestUrl(input), base);
    return isLoopbackHost(url.hostname) ? "loopback" : "local";
  } catch {
    return "local";
  }
}

export function localNetworkFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  return fetch(input, {
    ...init,
    targetAddressSpace: targetAddressSpaceFor(input),
  } as LocalNetworkRequestInit);
}
