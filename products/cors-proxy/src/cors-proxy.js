const DEFAULT_ALLOW_ORIGINS = "*";
const DEFAULT_ALLOWED_HOSTS = "github.com";

const allowHeaders = [
  "accept-encoding",
  "accept-language",
  "accept",
  "access-control-allow-origin",
  "authorization",
  "cache-control",
  "connection",
  "content-length",
  "content-type",
  "dnt",
  "git-protocol",
  "pragma",
  "range",
  "referer",
  "user-agent",
  "x-authorization",
  "x-http-method-override",
  "x-requested-with",
];

const exposeHeaders = [
  "accept-ranges",
  "age",
  "cache-control",
  "content-length",
  "content-language",
  "content-type",
  "date",
  "etag",
  "expires",
  "last-modified",
  "location",
  "pragma",
  "server",
  "transfer-encoding",
  "vary",
  "x-github-request-id",
  "x-redirected-url",
];

const allowMethods = ["POST", "GET", "OPTIONS"];
const maxAge = 60 * 60 * 24;

function splitList(value, fallback = "") {
  return (value || fallback)
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function corsOrigin(request, env) {
  const origins = splitList(
    env.ALLOW_ORIGINS ?? env.ALLOW_ORIGIN,
    DEFAULT_ALLOW_ORIGINS,
  );
  if (origins.includes("*")) return "*";

  const requestOrigin = request.headers.get("origin");
  if (requestOrigin && origins.includes(requestOrigin)) return requestOrigin;

  return origins[0] ?? "*";
}

function corsHeaders(request, env, preflight = false) {
  const headers = new Headers({
    "access-control-allow-origin": corsOrigin(request, env),
    "access-control-expose-headers": exposeHeaders.join(","),
  });

  if (preflight) {
    headers.set("access-control-allow-methods", allowMethods.join(","));
    headers.set("access-control-allow-headers", allowHeaders.join(","));
    headers.set("access-control-max-age", String(maxAge));
  }

  return headers;
}

export function isAllowedGitRequest(method, headers, url) {
  const isInfoRefs =
    url.pathname.endsWith("/info/refs") &&
    (url.searchParams.get("service") === "git-upload-pack" ||
      url.searchParams.get("service") === "git-receive-pack");

  switch (method) {
    case "OPTIONS":
      return true;
    case "POST": {
      const contentType = headers.get("content-type");
      return (
        (contentType === "application/x-git-upload-pack-request" &&
          url.pathname.endsWith("git-upload-pack")) ||
        (contentType === "application/x-git-receive-pack-request" &&
          url.pathname.endsWith("git-receive-pack"))
      );
    }
    case "GET":
      return isInfoRefs;
    default:
      return false;
  }
}

function parseProxyTarget(url, env) {
  const match = url.pathname.match(/^\/([^/]+)\/(.+)$/);
  if (!match) return null;

  const [, host, path] = match;
  const allowedHosts = splitList(env.ALLOWED_HOSTS, DEFAULT_ALLOWED_HOSTS);
  if (!allowedHosts.includes(host)) {
    return { error: "upstream host is not allowed" };
  }

  const insecureOrigins = splitList(env.INSECURE_HTTP_ORIGINS);
  const protocol = insecureOrigins.includes(host) ? "http" : "https";
  return { url: `${protocol}://${host}/${path}${url.search}` };
}

function proxyRequestHeaders(request) {
  const headers = new Headers();
  for (const headerName of allowHeaders) {
    const value = request.headers.get(headerName);
    if (value) headers.set(headerName, value);
  }

  const userAgent = headers.get("user-agent");
  if (!userAgent?.startsWith("git/")) {
    headers.set("user-agent", "git/@isomorphic-git/cors-proxy");
  }

  return headers;
}

function responseHeaders(upstreamHeaders, request, env, redirectedUrl) {
  const headers = corsHeaders(request, env);

  for (const headerName of exposeHeaders) {
    if (headerName === "content-length") continue;
    const value = upstreamHeaders.get(headerName);
    if (!value) continue;

    if (headerName === "location") {
      headers.set("location", value.replace(/^https?:\//, ""));
    } else {
      headers.set(headerName, value);
    }
  }

  if (redirectedUrl) {
    headers.set("x-redirected-url", redirectedUrl);
  }

  return headers;
}

export async function handleCorsProxy(request, env = {}, ctx = {}) {
  const url = new URL(request.url);
  const preflight = request.method === "OPTIONS";

  if (url.pathname === "/health") {
    return Response.json(
      { ok: true, service: "gitim-cors-proxy" },
      { headers: corsHeaders(request, env) },
    );
  }

  if (preflight) {
    return new Response(null, {
      status: 200,
      headers: corsHeaders(request, env, true),
    });
  }

  if (url.pathname === "/") {
    return new Response("GitIM CORS proxy", {
      status: 400,
      headers: corsHeaders(request, env),
    });
  }

  if (!isAllowedGitRequest(request.method, request.headers, url)) {
    return new Response(null, { status: 403, headers: corsHeaders(request, env) });
  }

  const target = parseProxyTarget(url, env);
  if (!target) {
    return new Response(null, { status: 403, headers: corsHeaders(request, env) });
  }
  if (target.error) {
    return new Response(target.error, {
      status: 403,
      headers: corsHeaders(request, env),
    });
  }

  const fetchImpl = ctx.fetch ?? fetch;
  try {
    const upstream = await fetchImpl(target.url, {
      method: request.method,
      redirect: "manual",
      headers: proxyRequestHeaders(request),
      body:
        request.method !== "GET" && request.method !== "HEAD"
          ? request.body
          : undefined,
    });

    return new Response(upstream.body, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: responseHeaders(
        upstream.headers,
        request,
        env,
        upstream.redirected ? upstream.url : null,
      ),
    });
  } catch {
    return new Response(null, { status: 502, headers: corsHeaders(request, env) });
  }
}
