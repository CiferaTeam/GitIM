import initWasm, { githubIdentityFromUserJson } from "gitim-wasm";

interface FetchLike {
  (input: RequestInfo | URL, init?: RequestInit): Promise<Response>;
}

export interface BrowserIdentity {
  handler: string;
  displayName: string;
  email: string | null;
}

interface InferBrowserIdentityOptions {
  remoteUrl: string;
  token: string;
  fetcher?: FetchLike;
}

interface WasmIdentity {
  handler: string;
  display_name: string;
  email?: string | null;
}

let wasmReady: Promise<void> | null = null;

async function ensureWasmReady(): Promise<void> {
  wasmReady ??= initWasm().then(() => undefined);
  await wasmReady;
}

function assertGithubRemote(remoteUrl: string): void {
  let url: URL;
  try {
    url = new URL(remoteUrl);
  } catch {
    throw new Error("Git remote URL must be a valid https://github.com/... URL");
  }
  if (url.protocol !== "https:" || url.hostname.toLowerCase() !== "github.com") {
    throw new Error("Browser mode identity inference currently supports github.com remotes");
  }
}

export async function inferBrowserIdentity({
  remoteUrl,
  token,
  fetcher = fetch,
}: InferBrowserIdentityOptions): Promise<BrowserIdentity> {
  assertGithubRemote(remoteUrl);
  const res = await fetcher("https://api.github.com/user", {
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `token ${token}`,
    },
  });

  if (!res.ok) {
    const detail = await res.text().catch(() => "");
    throw new Error(
      `GitHub identity inference failed: HTTP ${res.status}${detail ? ` ${detail}` : ""}`,
    );
  }

  const body = await res.text();
  await ensureWasmReady();
  const identity = githubIdentityFromUserJson(body) as WasmIdentity;
  return {
    handler: identity.handler,
    displayName: identity.display_name,
    email: identity.email ?? null,
  };
}
