type LocalNetworkRequestInit = RequestInit & {
  targetAddressSpace?: "local";
};

export function localNetworkFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  return fetch(input, {
    ...init,
    targetAddressSpace: "local",
  } as LocalNetworkRequestInit);
}
