import assert from "node:assert/strict";
import test from "node:test";

import { handleCorsProxy, isAllowedGitRequest } from "../src/cors-proxy.js";

function request(path, init = {}) {
  return new Request(`https://proxy.example${path}`, init);
}

test("health check returns JSON and configured CORS origin", async () => {
  const res = await handleCorsProxy(
    request("/health", { headers: { Origin: "https://gitim.io" } }),
    { ALLOW_ORIGINS: "https://gitim.io", ALLOWED_HOSTS: "github.com" },
  );

  assert.equal(res.status, 200);
  assert.equal(res.headers.get("access-control-allow-origin"), "https://gitim.io");
  assert.deepEqual(await res.json(), { ok: true, service: "gitim-cors-proxy" });
});

test("allows upstream git info refs requests", () => {
  const url = new URL("https://proxy.example/github.com/team/repo.git/info/refs?service=git-upload-pack");

  assert.equal(isAllowedGitRequest("GET", new Headers(), url), true);
});

test("blocks non git-like requests before upstream fetch", async () => {
  let fetched = false;
  const res = await handleCorsProxy(
    request("/github.com/team/repo.git/README.md"),
    { ALLOWED_HOSTS: "github.com" },
    {
      fetch: async () => {
        fetched = true;
        return new Response("unexpected");
      },
    },
  );

  assert.equal(res.status, 403);
  assert.equal(fetched, false);
});

test("blocks upstream hosts outside ALLOWED_HOSTS", async () => {
  let fetched = false;
  const res = await handleCorsProxy(
    request("/gitlab.com/team/repo.git/info/refs?service=git-upload-pack"),
    { ALLOWED_HOSTS: "github.com" },
    {
      fetch: async () => {
        fetched = true;
        return new Response("unexpected");
      },
    },
  );

  assert.equal(res.status, 403);
  assert.equal(await res.text(), "upstream host is not allowed");
  assert.equal(fetched, false);
});

test("proxies git GET requests with allowed headers and exposed response headers", async () => {
  const calls = [];
  const res = await handleCorsProxy(
    request("/github.com/team/repo.git/info/refs?service=git-upload-pack", {
      headers: {
        Accept: "application/x-git-upload-pack-advertisement",
        Authorization: "Basic abc",
        Origin: "https://gitim.io",
      },
    }),
    { ALLOW_ORIGINS: "https://gitim.io", ALLOWED_HOSTS: "github.com" },
    {
      fetch: async (url, init) => {
        calls.push({ url: String(url), init });
        return new Response("001e# service=git-upload-pack\n", {
          status: 200,
          headers: {
            "content-type": "application/x-git-upload-pack-advertisement",
            "x-github-request-id": "abc123",
          },
        });
      },
    },
  );

  assert.equal(res.status, 200);
  assert.equal(await res.text(), "001e# service=git-upload-pack\n");
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://github.com/team/repo.git/info/refs?service=git-upload-pack");
  assert.equal(calls[0].init.method, "GET");
  assert.equal(calls[0].init.headers.get("authorization"), "Basic abc");
  assert.equal(calls[0].init.headers.get("user-agent"), "git/@isomorphic-git/cors-proxy");
  assert.equal(res.headers.get("content-type"), "application/x-git-upload-pack-advertisement");
  assert.equal(res.headers.get("x-github-request-id"), "abc123");
  assert.equal(res.headers.get("access-control-expose-headers")?.includes("x-github-request-id"), true);
});

test("proxies git POST requests with request body", async () => {
  const calls = [];
  const res = await handleCorsProxy(
    request("/github.com/team/repo.git/git-upload-pack", {
      method: "POST",
      headers: { "content-type": "application/x-git-upload-pack-request" },
      body: "0032want abc\n0000",
    }),
    { ALLOWED_HOSTS: "github.com" },
    {
      fetch: async (url, init) => {
        calls.push({ url: String(url), init, body: await new Response(init.body).text() });
        return new Response("0000", { status: 200 });
      },
    },
  );

  assert.equal(res.status, 200);
  assert.equal(calls[0].url, "https://github.com/team/repo.git/git-upload-pack");
  assert.equal(calls[0].init.method, "POST");
  assert.equal(calls[0].body, "0032want abc\n0000");
});

test("rewrites upstream location headers to stay under the proxy", async () => {
  const res = await handleCorsProxy(
    request("/github.com/team/repo.git/info/refs?service=git-upload-pack"),
    { ALLOWED_HOSTS: "github.com" },
    {
      fetch: async () =>
        new Response("", {
          status: 302,
          headers: { location: "https://github.com/team/renamed.git/info/refs?service=git-upload-pack" },
        }),
    },
  );

  assert.equal(res.status, 302);
  assert.equal(
    res.headers.get("location"),
    "/github.com/team/renamed.git/info/refs?service=git-upload-pack",
  );
});

test("responds to CORS preflight without upstream fetch", async () => {
  let fetched = false;
  const res = await handleCorsProxy(
    request("/github.com/team/repo.git/git-upload-pack", {
      method: "OPTIONS",
      headers: {
        Origin: "https://gitim.io",
        "access-control-request-method": "POST",
        "access-control-request-headers": "content-type,authorization",
      },
    }),
    { ALLOW_ORIGINS: "https://gitim.io", ALLOWED_HOSTS: "github.com" },
    {
      fetch: async () => {
        fetched = true;
        return new Response("unexpected");
      },
    },
  );

  assert.equal(res.status, 200);
  assert.equal(res.headers.get("access-control-allow-methods"), "POST,GET,OPTIONS");
  assert.equal(fetched, false);
});
