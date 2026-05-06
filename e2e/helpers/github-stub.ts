import http from "node:http";
import type { AddressInfo } from "node:net";

export interface StubOptions {
  /** /user endpoint response. If `status` set, returns that status with empty body. */
  user?: { status?: number; body?: Record<string, unknown> };
  /** /repos/:owner/:repo endpoint response. */
  repo?: { status?: number; body?: Record<string, unknown> };
}

export interface StubServer {
  baseUrl: string;
  close: () => Promise<void>;
  /** Hits observed, useful for assertions in tests. */
  hits: string[];
}

/**
 * Start a minimal http server that mimics the two github REST endpoints the
 * runtime talks to: GET /user and GET /repos/:owner/:repo. Binds to 127.0.0.1
 * on an OS-assigned port so parallel tests don't clash.
 */
export async function startStubGithubApi(opts: StubOptions = {}): Promise<StubServer> {
  const hits: string[] = [];
  const userStatus = opts.user?.status ?? 200;
  const userBody = opts.user?.body ?? { login: "testuser", name: "Test User", id: 1 };
  const repoStatus = opts.repo?.status ?? 200;
  const repoBody = opts.repo?.body ?? { full_name: "fake/fake", private: false };

  const server = http.createServer((req, res) => {
    const url = req.url ?? "";
    hits.push(`${req.method} ${url}`);

    res.setHeader("Content-Type", "application/json");

    if (url === "/user") {
      res.statusCode = userStatus;
      res.end(userStatus === 200 ? JSON.stringify(userBody) : "");
      return;
    }
    if (/^\/repos\/[^/]+\/[^/]+$/.test(url)) {
      res.statusCode = repoStatus;
      res.end(repoStatus === 200 ? JSON.stringify(repoBody) : "");
      return;
    }
    res.statusCode = 404;
    res.end(JSON.stringify({ message: "Not Found" }));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const addr = server.address() as AddressInfo;
  const baseUrl = `http://127.0.0.1:${addr.port}`;

  return {
    baseUrl,
    hits,
    close: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}
